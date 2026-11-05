//! The graphics card, for the views the app draws itself (the galaxy, the arrangement, the sounds' waveforms). Slint
//! draws the window with wgpu, and once the window is up it hands over the same device; each view draws into a
//! picture on the card that Slint shows like any other image. Where there's no card to draw with (an old driver,
//! the screenshots, the tests, or `MO_DRAW_CPU=1` to compare), the views draw on the processor instead, as before.

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::rc::Rc;
use std::sync::Mutex;
use std::task::{Context, Poll, Waker};
use std::thread::LocalKey;
use std::time::{Duration, Instant};

use mo_app::{background, log};
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
    /// What waits for the next frame to be drawn: a drag out waits until its thing is shown dimmed.
    static AFTER_FRAME: RefCell<Vec<Once>> = const { RefCell::new(Vec::new()) };
    /// Counts the frames drawn with the graphics card, so a drag out can tell whether one has been drawn since.
    static FRAMES: Cell<u64> = const { Cell::new(0) };
}

/// How many frames the graphics card has drawn so far (none at all when the window isn't drawn with it).
pub fn frames_drawn() -> u64 {
    FRAMES.with(Cell::get)
}

/// Something to run once, by whichever comes first: the frame or the timer.
type Once = Rc<Cell<Option<Box<dyn FnOnce()>>>>;

/// Runs `f` on the window's thread once the next frame has been drawn and put on screen, or after `latest` if no
/// frame comes before then (nothing changed on screen, or the window isn't drawn with the graphics card), whichever is
/// first. Only once.
pub fn after_next_frame(latest: Duration, f: impl FnOnce() + 'static) {
    let once: Once = Rc::new(Cell::new(Some(Box::new(f))));
    AFTER_FRAME.with(|q| {
        let mut q = q.borrow_mut();
        // Those their timer ran already go, in case no frames come at all (the window isn't drawn with the card).
        q.retain(|w| {
            let f = w.take();
            let waiting = f.is_some();
            w.set(f);
            waiting
        });
        q.push(once.clone());
    });
    slint::Timer::single_shot(latest, move || {
        if let Some(f) = once.take() {
            f();
        }
    });
}

/// The card, looked for on a thread of its own from the very start (see [`look_ahead`]), while the log and the
/// settings start.
static AHEAD: Mutex<Option<std::thread::JoinHandle<Found>>> = Mutex::new(None);

/// What looking for the card found.
enum Found {
    /// `MO_DRAW_CPU` is set.
    Asked,
    /// A card, ready for Slint: the window draws with these, not with a second set Slint would make for itself. And
    /// how long each step of finding it took, for the log.
    Card(Box<WGPUConfiguration>, String),
    /// Only a software card: the views draw on the processor.
    Software,
    /// None found here (why): Slint looks for one itself, as it did before.
    NoCard(String),
}

/// Starts looking for the graphics card on a thread of its own, the first thing the app does. Finding one and making
/// its device takes a while on Windows (loading the drivers), and nothing else needs to wait for it until the window
/// is made ([`choose`]).
pub fn look_ahead() {
    let found = std::thread::Builder::new().name("Graphics card".to_owned()).spawn(find);
    if let (Ok(found), Ok(mut ahead)) = (found, AHEAD.lock()) {
        *ahead = Some(found);
    }
}

/// Looks for the fastest card there is and makes its device, once, for Slint to draw the window with (before, wgpu
/// was started three times: here, and twice by Slint). Only the card's own interfaces (Vulkan, DirectX 12, Metal) are
/// tried, not OpenGL: making an OpenGL one on Windows opens a hidden window of its own, and Slint avoids it for this
/// renderer anyway. Where none of them answers, Slint looks itself, as before. The device stays for the whole run. Debug builds don't turn the
/// drivers' own checks on (`WGPU_VALIDATION=1` still does): those slow every start and frame, and wgpu checks what
/// the app asks of the card itself either way.
fn find() -> Found {
    if std::env::var_os("MO_DRAW_CPU").is_some() {
        return Found::Asked;
    }
    // Slint's own way when a card is named (`WGPU_ADAPTER_NAME`): it picks that one.
    if std::env::var_os("WGPU_ADAPTER_NAME").is_some() {
        return Found::NoCard("a card is named in WGPU_ADAPTER_NAME".to_owned());
    }
    // One interface at a time, the one the card is usually drawn with first: starting each of them wakes its own
    // drivers (on Windows, Vulkan's and DirectX 12's both), and that's most of the time a start waits for the card.
    let tries: &[wgpu::Backends] =
        if cfg!(windows) { &[wgpu::Backends::VULKAN, wgpu::Backends::DX12] } else { &[wgpu::Backends::PRIMARY] };
    let mut steps: Vec<String> = Vec::new();
    let mut found = Found::NoCard("no graphics interface answered".to_owned());
    let mut card = None;
    for &backends in tries {
        let mut asked = wgpu::InstanceDescriptor::new_without_display_handle();
        asked.backends = backends;
        asked.flags = wgpu::InstanceFlags::VALIDATION_INDIRECT_CALL;
        let asked = asked.with_env();
        let name = match asked.backends {
            wgpu::Backends::VULKAN => "Vulkan",
            wgpu::Backends::DX12 => "DirectX 12",
            wgpu::Backends::METAL => "Metal",
            _ => "wgpu",
        };
        let began = Instant::now();
        let instance = wgpu::Instance::new(asked);
        let started = began.elapsed();
        let options = wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        };
        let picked = wait(instance.request_adapter(&options));
        steps.push(format!(
            "{name} started in {} ms, a card picked in {} ms",
            started.as_millis(),
            (began.elapsed() - started).as_millis()
        ));
        match picked {
            // A software card only when asked (`SLINT_WGPU_CPU`, as Slint itself takes it): the benches draw with one.
            Ok(adapter)
                if adapter.get_info().device_type != wgpu::DeviceType::Cpu
                    || std::env::var_os("SLINT_WGPU_CPU").is_some() =>
            {
                card = Some((instance, adapter));
                break;
            }
            Ok(_) => found = Found::Software,
            // None of them (an old card, with only OpenGL): Slint looks, as it did before.
            Err(e) => {
                if !matches!(found, Found::Software) {
                    found = Found::NoCard(e.to_string());
                }
            }
        }
        // `WGPU_BACKEND` names the interfaces itself: every try would be the same one.
        if std::env::var_os("WGPU_BACKEND").is_some() {
            break;
        }
    }
    let Some((instance, adapter)) = card else { return found };
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
    let began = Instant::now();
    let made = wait(adapter.request_device(&wanted));
    steps.push(format!("its device made in {} ms", began.elapsed().as_millis()));
    match made {
        Ok((device, queue)) => {
            Found::Card(Box::new(WGPUConfiguration::Manual { instance, adapter, device, queue }), steps.join(", "))
        }
        Err(e) => Found::NoCard(format!("the card wouldn't give a device ({e})")),
    }
}

/// Asks Slint to draw the window with wgpu, before any window is made, on the card [`look_ahead`] found (or finds it
/// now, if that didn't start). When there's no card to draw with, or `MO_DRAW_CPU` is set, the window is drawn the
/// usual way and the views draw on the processor.
pub fn choose() {
    let ahead = AHEAD.lock().ok().and_then(|mut a| a.take());
    let found = ahead.and_then(|f| f.join().ok()).unwrap_or_else(find);
    let config = match found {
        Found::Asked => {
            log::info("Graphics", "MO_DRAW_CPU is set: the views draw on the processor");
            return;
        }
        Found::Software => {
            log::warning("Graphics", "Only a software graphics card here, so the views draw on the processor");
            return;
        }
        Found::Card(config, steps) => {
            log::info("Graphics", &format!("Found the graphics card: {steps}"));
            *config
        }
        Found::NoCard(why) => {
            log::info("Graphics", &format!("Slint looks for the graphics card itself: {why}"));
            let mut settings = WGPUSettings::default();
            settings.power_preference = wgpu::PowerPreference::HighPerformance;
            WGPUConfiguration::Automatic(settings)
        }
    };
    if let Err(e) = slint::BackendSelector::new().require_wgpu_30(config).select() {
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
                    crate::piano_roll::graphics_ready(&app);
                    crate::waves::graphics_ready(&app);
                }
            });
        }
        // Frame timing (the `[Frames]` lines): how long Slint takes to lay out and draw the window, marked for the
        // freeze watchdog too. Only measures; it changes nothing that's drawn. The waveforms' time to draw new
        // pictures in starts again with each frame.
        RenderingState::BeforeRendering => {
            DRAWING_SINCE.with(|d| d.set(Some(std::time::Instant::now())));
            crate::watchdog::drawing(true);
            crate::waves::frame_starts();
        }
        RenderingState::AfterRendering => {
            crate::watchdog::drawing(false);
            if let Some(since) = DRAWING_SINCE.with(Cell::take) {
                mo_app::frame_timing::drawn(since.elapsed(), crate::page_name());
            }
            FRAMES.with(|f| f.set(f.get() + 1));
            crate::startup::frame_drawn();
            // Not from inside Slint's drawing: at the event loop's next turn, once this frame is on screen.
            let waiting = AFTER_FRAME.with(|q| std::mem::take(&mut *q.borrow_mut()));
            if !waiting.is_empty() {
                slint::Timer::single_shot(Duration::ZERO, move || {
                    for f in waiting.iter().filter_map(|w| w.take()) {
                        f();
                    }
                });
            }
        }
        RenderingState::RenderingTeardown => {
            GPU.with(|g| *g.borrow_mut() = None);
            GENERATION.with(|n| n.set(n.get() + 1));
            // What the views made on the card goes with it, now, while wgpu is still here: even as the window itself
            // goes, when there's no window left to show anything on.
            crate::sounds::card_gone();
            crate::arrangement::card_gone();
            crate::piano_roll::card_gone();
            crate::waves::card_gone();
            // The pictures on show go too, and are drawn again on the next card or the processor.
            if let Some(app) = weak.upgrade() {
                crate::sounds::graphics_gone(&app);
                crate::arrangement::graphics_gone(&app);
                crate::piano_roll::graphics_gone(&app);
                crate::waves::graphics_gone(&app);
            }
        }
        _ => {}
    });
    if let Err(e) = set {
        log::info("Graphics", &format!("No graphics card to draw the views with: {e:?}"));
        // Nothing says when a frame is drawn then: the start-up line goes at the event loop's first turn, as near
        // as it gets (the window is up by then).
        slint::Timer::single_shot(Duration::ZERO, crate::startup::frame_drawn);
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

/// A view's painter on the graphics card (the galaxy's, the arrangement's, the waveforms'): made on a card when the
/// card is handed over ([`prepare`]) or the first time the view is drawn on it, gone with that card, and not made again
/// on a card that refused to draw the view. On that card the processor draws the view; the next card is tried again.
/// Each view keeps one, on the window's thread, for as long as the app is open.
pub struct Painter<T> {
    /// The painter, and which card it was made on (as [`generation`] counts).
    made: Option<(u64, T)>,
    /// The card that refused to draw the view.
    refused: Option<u64>,
    /// The card a painter is being made on away from the window's thread ([`prepare`]).
    preparing: Option<u64>,
}

impl<T> Painter<T> {
    pub const fn new() -> Painter<T> {
        Painter { made: None, refused: None, preparing: None }
    }

    /// The card to draw the view on now: the one there is, unless it refused to, or its painter is still being made away
    /// from the window's thread (the processor draws the view until it's ready).
    pub fn card(&self) -> Option<(Gpu, u64)> {
        current().filter(|(_, on)| self.draws_on(*on))
    }

    /// Whether the view is drawn on card `on` now (see [`Painter::card`]).
    fn draws_on(&self, on: u64) -> bool {
        self.refused != Some(on) && self.preparing != Some(on)
    }

    /// The painter on card `on` (`gpu`): the one made on it already, or one `make` makes now. `Err` says why the card
    /// wouldn't make it.
    pub fn on(&mut self, gpu: &Gpu, on: u64, make: impl FnOnce(&Gpu) -> T) -> Result<&mut T, String> {
        let made = match self.made.take() {
            Some((made_on, painter)) if made_on == on => (on, painter),
            old => {
                // None made yet, or one made on a card that's gone, which goes first.
                drop(old);
                (on, checked(&gpu.device, || make(gpu))?)
            }
        };
        Ok(&mut self.made.insert(made).1)
    }

    /// The painter, if one has been made.
    pub fn made(&mut self) -> Option<&mut T> {
        self.made.as_mut().map(|(_, painter)| painter)
    }

    /// Whether the painter has been made on card `on`.
    pub fn made_on(&self, on: u64) -> bool {
        self.made.as_ref().is_some_and(|(made_on, _)| *made_on == on)
    }

    /// Card `on` refused to draw the view: its painter goes, and the processor draws the view while that card lasts.
    pub fn refused(&mut self, on: u64) {
        self.refused = Some(on);
        if self.made_on(on) {
            self.made = None;
        }
    }

    /// The card went: the painter goes with it.
    pub fn gone(&mut self) {
        self.made = None;
    }

    /// Whether a painter for card `on` is still wanted: it's the card there is now, it hasn't refused, and no painter has
    /// been made on it yet.
    fn wants(&self, on: u64) -> bool {
        on == generation() && self.refused != Some(on) && !self.made_on(on)
    }
}

impl<T> Default for Painter<T> {
    fn default() -> Painter<T> {
        Painter::new()
    }
}

/// Makes a view's painter (with `make`) on the card there is now, away from the window's thread: building its shader
/// can take the card a moment, which the window shouldn't wait for. Meanwhile the processor draws the view; once the
/// painter is in `slot` (the view's), `ready` draws the view again, on the card. If the card says no, the processor
/// goes on drawing the view while that card lasts, as when it refuses to draw. `name` is the view's, for the log.
pub fn prepare<T: Send + 'static>(
    app: &AppWindow,
    name: &'static str,
    slot: &'static LocalKey<RefCell<Painter<T>>>,
    make: impl FnOnce(&Gpu) -> T + Send + 'static,
    ready: fn(&AppWindow),
) {
    let Some((gpu, on)) = current() else { return };
    if slot.with(|p| {
        let p = p.borrow();
        !p.wants(on) || p.preparing == Some(on)
    }) {
        return;
    }
    let weak = app.as_weak();
    let started = background::spawn(&format!("{name} painter"), move || {
        let on_its_way = OnItsWay(Some(checked(&gpu.device, || make(&gpu))));
        // Kept on the window's thread, where the view draws, if it's still wanted there.
        let _ = slint::invoke_from_event_loop(move || {
            let mut on_its_way = on_its_way;
            let Some(made) = on_its_way.0.take() else { return };
            slot.with(|p| {
                let mut p = p.borrow_mut();
                if p.preparing == Some(on) {
                    p.preparing = None;
                }
                match made {
                    Ok(painter) if p.wants(on) => p.made = Some((on, painter)),
                    // Not wanted any more (the card went meanwhile), or the card that said no has gone already.
                    Ok(_) => {}
                    Err(_) if on != generation() => {}
                    Err(e) => {
                        log::error(
                            name,
                            &format!("The graphics card couldn't set up its painter, so the processor draws it: {e}"),
                        );
                        p.refused(on);
                    }
                }
            });
            if let Some(app) = weak.upgrade() {
                ready(&app);
            }
        });
    });
    match started {
        Ok(_) => slot.with(|p| p.borrow_mut().preparing = Some(on)),
        // The view makes it itself, when it's first drawn on the card.
        Err(e) => log::info(name, &format!("Couldn't set up the graphics card's painter ahead of time: {e}")),
    }
}

/// A painter (or why there's none) on its way to the window's thread. If the window has closed and its event loop
/// ended by then, the message is never run: Slint keeps it, and lets it go only as the app's thread ends, along with
/// that thread's own stores, after wgpu's (in a debug build, wgpu then stops the app with an error). So a painter that
/// never arrives isn't dropped, just left for the system to clear away with the rest of the app, which is ending.
struct OnItsWay<T>(Option<T>);

impl<T> Drop for OnItsWay<T> {
    fn drop(&mut self) {
        // Taken out when it arrives; still here only if it never did.
        if let Some(never_arrived) = self.0.take() {
            std::mem::forget(never_arrived);
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// What a view keeps of its painter, here without a card (the tests have none): the painter made on a card stays
    /// until that card goes or refuses, and none is wanted for a card that isn't here, already has one, or refused.
    #[test]
    fn a_painter_goes_with_its_card() {
        let mut p: Painter<&str> = Painter::new();
        assert!(p.card().is_none(), "no card in the tests");
        let now = generation();
        assert!(p.wants(now) && p.draws_on(now));
        assert!(!p.wants(now + 1), "not for a card that isn't here");
        p.made = Some((now, "painter"));
        assert!(!p.wants(now), "one made already");
        assert_eq!(p.made().copied(), Some("painter"));
        p.gone();
        assert!(p.made().is_none());
        assert!(p.wants(now), "wanted again once the card comes back");
        p.made = Some((now, "painter"));
        p.refused(now);
        assert!(p.made().is_none());
        assert!(!p.wants(now) && !p.draws_on(now), "not on a card that refused");
        assert!(p.draws_on(now + 1), "the next card is tried again");
        // A card that's gone refusing late leaves the next card's painter be.
        p.made = Some((now + 1, "next"));
        p.refused(now);
        assert_eq!(p.made().copied(), Some("next"));
    }

    /// A painter that reaches the window's thread is taken and used (dropped as usual when it's let go); one that
    /// never gets there (the window closed first) isn't dropped at all.
    #[test]
    fn a_painter_that_never_arrives_is_left_alone() {
        struct Painter(std::rc::Rc<Cell<bool>>);
        impl Drop for Painter {
            fn drop(&mut self) {
                self.0.set(true);
            }
        }
        let dropped = std::rc::Rc::new(Cell::new(false));
        let mut arrived = OnItsWay(Some(Painter(dropped.clone())));
        drop(arrived.0.take());
        drop(arrived);
        assert!(dropped.get(), "used and let go");
        let dropped = std::rc::Rc::new(Cell::new(false));
        drop(OnItsWay(Some(Painter(dropped.clone()))));
        assert!(!dropped.get(), "never arrived, so never dropped");
    }

    /// While a painter is being made away from the window's thread, the processor draws the view.
    #[test]
    fn the_processor_draws_until_the_painter_is_ready() {
        let mut p: Painter<&str> = Painter::new();
        let now = generation();
        p.preparing = Some(now);
        assert!(!p.draws_on(now));
        assert!(p.wants(now), "the painter being made is still wanted when it comes");
        p.preparing = None;
        p.made = Some((now, "painter"));
        assert!(p.draws_on(now));
    }

    /// A painter is made once on a card, and again on the next one. Needs a graphics card (a software one will do), so
    /// it's run by hand.
    #[test]
    #[ignore]
    fn a_painter_is_made_once_a_card() {
        let Some((device, queue)) = crate::canvas::testing::card() else {
            eprintln!("No graphics card here, not even a software one: nothing to make a painter on");
            return;
        };
        let gpu = Gpu { device, queue };
        let mut p: Painter<u32> = Painter::new();
        let mut made = 0;
        for _ in 0..3 {
            let painter = p.on(&gpu, 1, |_| {
                made += 1;
                made
            });
            assert_eq!(painter.copied(), Ok(1));
        }
        let painter = p.on(&gpu, 2, |_| {
            made += 1;
            made
        });
        assert_eq!(painter.copied(), Ok(2), "made again on the next card");
    }
}
