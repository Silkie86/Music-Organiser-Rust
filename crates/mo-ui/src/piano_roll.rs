//! Draws the MIDI page's piano roll off the window's thread (Solomon, 2026-11-04: what can slow the screens goes off
//! it). A background thread works out its picture (`mo_app::piano_roll`), and draws it there too when the processor
//! draws it (where there's no graphics card, or the card won't); the card draws it on the window's thread, which then
//! puts it on the page with the names laid on it. A new part, size, choice or look asks for a new picture, and the
//! newest ask wins (rule 12): a picture that arrives once a newer one is on its way, or another part is shown, is
//! passed over.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use mo_app::background::Newest;
use mo_app::log;
use mo_app::midi::Roll;
use mo_app::piano_roll::{self, Choices, Colours};
use mo_app::shapes::Colour;
use slint::{Color, ComponentHandle, Image, ModelRc, VecModel};

use crate::canvas::{self, Canvas, Kind};
use crate::gpu;
use crate::{AppWindow, MidiData, RollLabel, Theme};

/// A picture to work out: of which roll and which picture (both are counted), the roll, its size on the page in
/// points, its choices and colours, pixels to a point, and whether the card draws it (else the processor does, on the
/// drawing thread).
struct Ask {
    shown: u64,
    number: u64,
    roll: Arc<Roll>,
    size: (f64, f64),
    choices: Choices,
    colours: Colours,
    scale: f64,
    on_card: bool,
}

/// A worked out picture and its names, ready for the window: made on the drawing thread.
struct Drawn {
    drawing: canvas::Drawing,
    labels: Vec<RollLabel>,
}

#[derive(Default)]
struct State {
    /// Counts the rolls shown (a part's, or none while the next part's notes are read), so a picture of one shown
    /// before is dropped.
    shown: u64,
    /// Counts the pictures asked for.
    pictures: u64,
    /// The roll shown, if it has notes to draw.
    roll: Option<Arc<Roll>>,
    /// The roll's size on the page, in points.
    size: (f64, f64),
    /// Draw here and now, on the processor, instead: for the screenshots, which have no event loop to post to.
    here: bool,
}

/// The newest picture the drawing thread has sent the window (as `State::pictures` counts them): one older than that
/// is passed over when it gets there, as it would only be drawn over at once.
static SENT: AtomicU64 = AtomicU64::new(0);

/// What the roll's steps on the window's thread are called in the freeze lines.
const STEPS: canvas::Steps = canvas::Steps {
    set_up: "MIDI roll: set up the GPU painter",
    draw: "MIDI roll: draw on the GPU",
    show: "MIDI roll: show the CPU picture",
};

thread_local! {
    static STATE: RefCell<State> = RefCell::default();
    /// The names on the roll, kept and changed line by line, so a new picture doesn't make the window build them all
    /// again.
    static WORDS: Rc<VecModel<RollLabel>> = Rc::default();
    static DRAWER: RefCell<Option<Newest<Ask>>> = const { RefCell::new(None) };
    /// The painter on the graphics card.
    static CARD: RefCell<gpu::Painter<Canvas>> = const { RefCell::new(gpu::Painter::new()) };
}

/// Wires the roll to its size and choices, and starts its drawing thread.
pub fn connect(app: &AppWindow) {
    let data = app.global::<MidiData>();
    data.set_roll_labels(ModelRc::from(WORDS.with(Rc::clone)));
    let weak = app.as_weak();
    data.on_roll_laid_out(move |w, h| {
        STATE.with(|s| s.borrow_mut().size = (f64::from(w), f64::from(h)));
        if let Some(app) = weak.upgrade() {
            redraw(&app);
        }
    });
    let weak = app.as_weak();
    let drawer = Newest::start("MIDI roll drawing", move |ask: Ask| {
        let (shown, number) = (ask.shown, ask.number);
        let drawn = Drawn::new(&ask);
        SENT.fetch_max(number, Ordering::Relaxed);
        let _ = weak.upgrade_in_event_loop(move |app| put_up(&app, shown, Some(number), drawn));
    });
    match drawer {
        Ok(d) => DRAWER.with(|slot| *slot.borrow_mut() = Some(d)),
        Err(e) => log::error("MIDI", &format!("Couldn't start drawing the piano roll: {e}")),
    }
}

/// Shows the picked part's roll, or none (while the next part's notes are read, or when its notes can't be): the last
/// roll's picture goes at once, and the new one follows.
pub fn show(app: &AppWindow, roll: Option<Roll>) {
    let roll = roll.filter(|r| !r.notes.is_empty() && r.low <= r.high);
    let keys = roll.as_ref().map_or(0, |r| i32::from(r.high - r.low) + 1);
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.shown += 1;
        s.roll = roll.map(Arc::new);
    });
    let data = app.global::<MidiData>();
    data.set_roll_keys(keys);
    data.set_roll_picture(Image::default());
    WORDS.with(|w| canvas::refill(w, Vec::new()));
    redraw(app);
}

/// The window's theme has new colours: the roll is drawn again in them.
pub fn look_changed(app: &AppWindow) {
    redraw(app);
}

/// Draws here and now from now on: for the screenshots, which have no event loop to post to.
pub fn draw_here() {
    STATE.with(|s| s.borrow_mut().here = true);
}

/// Asks for a picture of the roll as it is now: for the graphics card when there's one to draw on.
fn redraw(app: &AppWindow) {
    let data = app.global::<MidiData>();
    let choices = Choices { keyboard: data.get_roll_keyboard(), names: data.get_roll_names() };
    let colours = colours(app);
    let scale = f64::from(app.window().scale_factor());
    let ask = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let roll = s.roll.clone()?;
        let (w, h) = s.size;
        (w - piano_roll::LEFT - piano_roll::TOP >= 1.0 && h - 2.0 * piano_roll::TOP >= 1.0).then_some(())?;
        s.pictures += 1;
        let ask =
            Ask { shown: s.shown, number: s.pictures, roll, size: s.size, choices, colours, scale, on_card: false };
        Some((ask, s.here))
    });
    let Some((mut ask, here)) = ask else { return };
    if here {
        let shown = ask.shown;
        put_up(app, shown, None, Drawn::new(&ask));
        return;
    }
    ask.on_card = CARD.with(|c| c.borrow().card()).is_some();
    DRAWER.with(|slot| {
        if let Some(drawer) = slot.borrow().as_ref() {
            drawer.ask(ask);
        }
    });
}

/// The roll's colours, as the window's theme has them now.
fn colours(app: &AppWindow) -> Colours {
    let t = app.global::<Theme>();
    // A piano's keys in either theme: white keys light and black keys dark.
    let (white_key, black_key) =
        if t.get_light() { (t.get_surface(), t.get_text()) } else { (t.get_muted(), t.get_background()) };
    let well = t.get_well();
    Colours {
        well: u32::from(well.red()) << 16 | u32::from(well.green()) << 8 | u32::from(well.blue()),
        shine: colour(t.get_shine()),
        edge: colour(t.get_edge()),
        white_key: colour(white_key),
        black_key: colour(black_key),
        accent: colour(t.get_accent()),
        accent_text: colour(t.get_accent_text()),
    }
}

fn colour(c: Color) -> Colour {
    Colour { r: c.red(), g: c.green(), b: c.blue(), a: c.alpha() }
}

impl Drawn {
    fn new(ask: &Ask) -> Drawn {
        let piano_roll::Picture { scene, labels } =
            piano_roll::picture(&ask.roll, ask.size, ask.scale, ask.choices, &ask.colours);
        let labels = labels
            .into_iter()
            .map(|l| RollLabel {
                x: l.x as f32,
                y: l.y as f32,
                width: l.width as f32,
                height: l.height as f32,
                text: l.text.into(),
                color: Color::from_argb_u8(l.colour.a, l.colour.r, l.colour.g, l.colour.b),
                key: l.key,
            })
            .collect();
        Drawn { drawing: canvas::Drawing::new(&scene, ask.scale, ask.on_card), labels }
    }
}

/// A picture has been worked out (picture `number`, or here and now): drawn on the card if it's for the card, and put
/// on the page with its names, unless another roll has been shown since or a newer picture is on its way already.
fn put_up(app: &AppWindow, shown: u64, number: Option<u64>, drawn: Drawn) {
    if STATE.with(|s| s.borrow().shown != shown) || number.is_some_and(|n| n < SENT.load(Ordering::Relaxed)) {
        return;
    }
    let data = app.global::<MidiData>();
    match canvas::show(drawn.drawing, &CARD, &STEPS) {
        Ok(Some(image)) => data.set_roll_picture(image),
        // The card went since this was asked for: drawn again, on the processor.
        Ok(None) => {
            redraw(app);
            return;
        }
        Err(e) => {
            log::error("MIDI", &format!("The graphics card couldn't draw the piano roll, so the processor does: {e}"));
            redraw(app);
            return;
        }
    }
    let _doing = crate::watchdog::doing("MIDI roll: place the names");
    WORDS.with(|w| canvas::refill(w, drawn.labels));
}

/// A graphics card was handed over (the window opened, or the card was reset): the painter is set up on it, away from
/// the window's thread, and the roll is drawn again on it once it's ready (by the processor until then).
pub fn graphics_ready(app: &AppWindow) {
    gpu::prepare(app, "MIDI roll", &CARD, |gpu| Canvas::new(&gpu.device, &gpu.queue, Kind::Picture), redraw);
    redraw(app);
}

/// The card went (the window closed, or the card was reset): the painter goes with it, at once, even as the window
/// itself goes.
pub fn card_gone() {
    CARD.with(|c| c.borrow_mut().gone());
}

/// The card went: the picture made on it isn't shown any more. It's drawn again on the next card, or on the processor.
pub fn graphics_gone(app: &AppWindow) {
    let weak = app.as_weak();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(app) = weak.upgrade() {
            app.global::<MidiData>().set_roll_picture(Image::default());
            redraw(&app);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::testing::{card, read_back};
    use mo_app::midi::RollNote;
    use mo_app::themes::Look;

    /// A made-up roll of four octaves: a run of notes up and down, long and short, soft and hard, some on top of each
    /// other, with a line at each bar.
    fn busy_roll() -> Roll {
        let (low, high) = (36u8, 83u8);
        let keys = f32::from(high - low + 1);
        let notes = (0..600u32)
            .map(|i| {
                let key = low + ((i * 7) % u32::from(high - low + 1)) as u8;
                RollNote {
                    x: (i as f32 * 0.0016) % 0.98,
                    y: f32::from(high - key) / keys,
                    w: 0.002 + (i % 9) as f32 * 0.004,
                    h: 1.0 / keys,
                    strength: 0.4 + 0.6 * ((i % 5) as f32 / 4.0),
                    key,
                }
            })
            .collect();
        Roll { notes, bars: (1..16).map(|b| b as f32 / 16.0).collect(), low, high, left_off: 0 }
    }

    /// The roll's colours in a look, near enough to the window's for drawing them both ways.
    fn colours(look: &Look) -> Colours {
        let t = &look.theme;
        let c = |rgb: u32| Colour::rgb(rgb, 1.0);
        Colours {
            well: t.background,
            shine: Colour::rgb(0xFFFFFF, if t.light { 0.5 } else { 0.07 }),
            edge: c(t.stroke),
            white_key: c(if t.light { t.surface } else { t.muted }),
            black_key: c(if t.light { t.text } else { t.background }),
            accent: c(look.accent),
            accent_text: c(look.accent_text),
        }
    }

    /// Draws a busy roll on whatever graphics card there is (a software one will do) and on the processor, in both
    /// kinds of theme, with and without its keyboard and names, at 1, 1.25, 1.5 and 2 pixels to a point, and checks the
    /// two pictures agree: the rows, keys and lines fill the very same pixels, and the notes differ only a little at
    /// their soft corners (at most 19 of 255 on llvmpipe, in under 1% of the pixels). With ROLL_GPU_LOOK set to a
    /// folder, writes both pictures there, to judge by eye.
    #[test]
    #[ignore]
    fn the_card_draws_what_the_processor_draws() {
        let Some((device, queue)) = card() else {
            eprintln!("No graphics card here, not even a software one: nothing to compare");
            return;
        };
        let out = std::env::var("ROLL_GPU_LOOK").ok();
        let mut canvas = Canvas::new(&device, &queue, Kind::Picture);
        let roll = busy_roll();
        let looks =
            [("graphite", Look::from_settings(None, None)), ("paper", Look::from_settings(Some("Paper"), None))];
        for (theme, look) in &looks {
            for scale in [1.0, 1.25, 1.5, 2.0] {
                for (keyboard, names) in [(true, false), (false, true)] {
                    let name = format!(
                        "{theme}-{scale}-{}{}",
                        if keyboard { "keyboard" } else { "plain" },
                        if names { "-names" } else { "" }
                    );
                    let choices = Choices { keyboard, names };
                    let picture = piano_roll::picture(&roll, (433.5, 520.0), scale, choices, &colours(look));
                    let scene = &picture.scene;
                    let cpu = scene.paint(scale);
                    let bytes = scene.pack(scale);
                    let texture = canvas.draw(scene.pixel_size(scale), scale as f32, scene.background, &bytes, 0.0);
                    let gpu = read_back(&device, &queue, &texture);
                    assert_eq!((gpu.width(), gpu.height()), (cpu.width(), cpu.height()), "{name}");
                    let (mut off, mut far) = (0usize, 0usize);
                    for (x, y, g) in gpu.enumerate_pixels() {
                        let c = cpu.pixel(x, y).expect("inside");
                        let d = [g[0].abs_diff(c.red()), g[1].abs_diff(c.green()), g[2].abs_diff(c.blue())];
                        let d = d.into_iter().max().unwrap_or(0);
                        off += usize::from(d > 8);
                        far += usize::from(d > 32);
                    }
                    let pixels = (gpu.width() * gpu.height()) as usize;
                    eprintln!(
                        "{name}: {} shapes, {off} of {pixels} pixels differ by more than 8, {far} by more than 32",
                        scene.shapes.len()
                    );
                    if let Some(out) = &out {
                        gpu.save(format!("{out}/roll-{name}-card.png")).expect("saved");
                        let cpu_png = image::RgbaImage::from_raw(cpu.width(), cpu.height(), cpu.data().to_vec())
                            .expect("same size");
                        cpu_png.save(format!("{out}/roll-{name}-processor.png")).expect("saved");
                    }
                    // A pixel far out would be a row, key, line or note in the wrong place or colour.
                    assert_eq!(far, 0, "{name}: {far} pixels differ by more than 32");
                    assert!(off * 100 <= pixels, "{name}: {off} pixels differ by more than 8");
                }
            }
        }
    }
}
