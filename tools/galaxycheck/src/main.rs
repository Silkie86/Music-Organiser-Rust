//! The phase 0 galaxy check (spec section 16): before anything is built on it, can a Slint window show 300,000 stars
//! drawn with wgpu at 60 frames a second, on Solomon's PC?
//!
//! `galaxy-check` opens a window and flies a scripted route for 18 seconds: the whole sky, flying in, then deep in
//! and moving across. It times every frame, then shows the numbers and writes them to
//! `logs\galaxy-check-<time>.txt` in the app's data folder, where Claude can read them. After that, drag to move and
//! scroll to zoom, to try it by hand. Nothing is read from or written to the library.

slint::include_modules!();

mod gpu;
mod sky;

use std::cell::RefCell;
use std::fmt::Write as _;
use std::rc::Rc;
use std::time::Instant;

use slint::wgpu_30::{WGPUConfiguration, WGPUSettings, wgpu};
use slint::{ComponentHandle, GraphicsAPI, RenderingState};

use gpu::StarPainter;
use sky::{STAGE_SECONDS, STAGES, Timings, View};

const STARS: usize = 300_000;

struct State {
    painter: Option<StarPainter>,
    graphics: String,
    started: Option<Instant>,
    last_frame: Option<Instant>,
    timings: Vec<Timings>,
    view: View,
    /// The scripted run is over and the numbers are written.
    finished: bool,
    size: (u32, u32),
    scale: f32,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut settings = WGPUSettings::default();
    settings.power_preference = wgpu::PowerPreference::HighPerformance;
    if let Err(e) = slint::BackendSelector::new().require_wgpu_30(WGPUConfiguration::Automatic(settings)).select() {
        let why = format!("Slint couldn't draw with wgpu on this PC: {e}");
        write_results(&why);
        return Err(why.into());
    }

    let app = CheckWindow::new()?;
    app.set_stage("Getting the graphics card ready…".into());
    let state = Rc::new(RefCell::new(State {
        painter: None,
        graphics: String::new(),
        started: None,
        last_frame: None,
        timings: vec![Timings::default(); STAGES.len()],
        view: View::default(),
        finished: false,
        size: (0, 0),
        scale: 1.0,
    }));

    let weak = app.as_weak();
    let st = state.clone();
    app.window().set_rendering_notifier(move |rendering, api| {
        let Some(app) = weak.upgrade() else { return };
        match rendering {
            RenderingState::RenderingSetup => {
                if let GraphicsAPI::WGPU30 { device, queue, .. } = api {
                    let info = device.adapter_info();
                    let mut st = st.borrow_mut();
                    st.graphics = format!(
                        "{} ({:?}, {:?}), driver {} {}",
                        info.name, info.backend, info.device_type, info.driver, info.driver_info
                    );
                    st.painter = Some(StarPainter::new(device, queue, &sky::make(STARS)));
                }
            }
            RenderingState::BeforeRendering => frame(&app, &mut st.borrow_mut()),
            RenderingState::RenderingTeardown => st.borrow_mut().painter = None,
            _ => {}
        }
    })?;

    let st = state.clone();
    app.on_dragged(move |dx, dy| {
        let mut st = st.borrow_mut();
        if !st.finished {
            return;
        }
        // Half the shorter side is one sky unit at zoom 1. The mouse moves in logical pixels; the picture is physical.
        let scale = st.scale;
        let fit = st.size.0.min(st.size.1).max(1) as f32 / 2.0 * st.view.zoom;
        st.view.x -= dx * scale / fit;
        st.view.y += dy * scale / fit;
    });
    let st = state.clone();
    app.on_scrolled(move |delta| {
        let mut st = st.borrow_mut();
        if st.finished {
            st.view.zoom = (st.view.zoom * (delta / 400.0).exp()).clamp(0.5, 256.0);
        }
    });

    app.run()?;
    Ok(())
}

/// One frame: times it, moves the camera, draws the stars and asks for the next frame straight away.
fn frame(app: &CheckWindow, st: &mut State) {
    let now = Instant::now();
    let started = *st.started.get_or_insert(now);
    let elapsed = now.duration_since(started).as_secs_f32();
    let interval = st.last_frame.map(|l| now.duration_since(l).as_secs_f32() * 1000.0);
    st.last_frame = Some(now);

    if !st.finished {
        match sky::scripted(elapsed) {
            Some((stage, view)) => {
                st.view = view;
                // The first half second of a stage is left out: the change of stage itself isn't what's measured.
                if let Some(ms) = interval
                    && elapsed % STAGE_SECONDS > 0.5
                {
                    st.timings[stage].frames_ms.push(ms);
                }
                app.set_stage(format!("{} ({} of {})", STAGES[stage], stage + 1, STAGES.len()).into());
            }
            None => {
                st.finished = true;
                let text = results(st);
                write_results(&text);
                println!("{text}");
                app.set_stage("Done. Drag to move, scroll to zoom.".into());
                app.set_results(text.into());
            }
        }
    }

    let scale = app.window().scale_factor();
    let size = ((app.get_sky_width() * scale) as u32, (app.get_sky_height() * scale) as u32);
    st.size = size;
    st.scale = scale;
    let view = st.view;
    let Some(painter) = st.painter.as_mut() else { return };
    let drawn = Instant::now();
    let texture = painter.draw(view, size.0, size.1, [0.043, 0.051, 0.071]);
    let cpu_ms = drawn.elapsed().as_secs_f32() * 1000.0;
    match slint::Image::try_from(texture) {
        Ok(image) => app.set_sky(image),
        Err(e) => app.set_stage(format!("Slint wouldn't show the picture: {e}").into()),
    }
    app.set_numbers(
        format!(
            "{STARS} stars, {}×{} px, zoom {:.1}\nThis frame: {} ms (sending it took {cpu_ms:.2} ms)\n{}",
            size.0,
            size.1,
            view.zoom,
            interval.map_or("–".into(), |ms| format!("{ms:.1}")),
            st.graphics
        )
        .into(),
    );
    app.window().request_redraw();
}

/// The numbers for each stage, and whether the galaxy can stay on Slint.
fn results(st: &State) -> String {
    let mut out = format!(
        "Galaxy check, {}\nGraphics: {}\nPicture: {}×{} px, {STARS} stars\n\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M"),
        st.graphics,
        st.size.0,
        st.size.1
    );
    let mut passed = true;
    for (name, t) in STAGES.iter().zip(&st.timings) {
        match t.summary() {
            Some((middle, p95, slowest, late)) => {
                passed &= p95 <= 17.5;
                let _ = writeln!(
                    out,
                    "{name}: middle {middle:.1} ms, 95% under {p95:.1} ms, slowest {slowest:.1} ms; {late} of {} frames late",
                    t.frames_ms.len()
                );
            }
            None => {
                passed = false;
                let _ = writeln!(out, "{name}: no frames");
            }
        }
    }
    out += if passed {
        "\nPassed: 60 frames a second in every stage. The galaxy can be drawn with wgpu inside Slint."
    } else {
        "\nFell short of 60 frames a second somewhere. Send this to Claude: the next step is egui or a window of its own."
    };
    out += "\n(Frame times can't go below the screen's own: 16.7 ms on a 60 Hz screen, 6.9 ms on 144 Hz.)";
    out
}

/// Writes the numbers beside the app's session logs, so Claude can read them from the PC.
fn write_results(text: &str) {
    let dir = mo_app::paths::logs_dir();
    let name = format!("galaxy-check-{}.txt", chrono::Local::now().format("%Y-%m-%d_%H-%M-%S"));
    if std::fs::create_dir_all(&dir).is_ok() {
        let _ = std::fs::write(dir.join(&name), text);
        println!("Written to {}", dir.join(name).display());
    }
}
