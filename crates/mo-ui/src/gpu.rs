//! The graphics card, for the views the app draws itself (the galaxy, then the arrangement and the waveforms). Slint
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
}

/// Asks Slint to draw the window with wgpu, before any window is made, on the fastest card there is. When that can't
/// be done, or `MO_DRAW_CPU` is set, the window is drawn the usual way and the views draw on the processor.
pub fn choose() {
    if std::env::var_os("MO_DRAW_CPU").is_some() {
        log::info("Graphics", "MO_DRAW_CPU is set: the views draw on the processor");
        return;
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
/// over, the views draw again on it.
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
                }
            });
        }
        RenderingState::RenderingTeardown => {
            GPU.with(|g| *g.borrow_mut() = None);
            GENERATION.with(|n| n.set(n.get() + 1));
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
    let invalid = now(valid.pop());
    let full = now(memory.pop());
    match invalid.or(full) {
        Some(e) => Err(e.to_string()),
        None => Ok(out),
    }
}

/// The answer of a future that's ready at once (wgpu's error scopes are, on a PC); `None` if it isn't.
fn now<T>(f: impl Future<Output = Option<T>>) -> Option<T> {
    let mut f = std::pin::pin!(f);
    match f.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(v) => v,
        Poll::Pending => None,
    }
}
