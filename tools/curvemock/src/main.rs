//! Draws the curve editor's design mock-ups to PNG files: `curvemock [folder]` (default `shots`). Each state sets the
//! mock page's properties (ui/curve-editor.slint) and saves what Slint's software renderer drew. Mock data only.

use std::path::PathBuf;
use std::rc::Rc;

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::{ComponentHandle, ModelRc, PhysicalSize, Rgb8Pixel, SharedString, VecModel};

slint::include_modules!();

struct Headless(Rc<MinimalSoftwareWindow>);

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

/// A point of a curve: time 0..1 across the picture, value 0..1, bend -1..1 on the line to the next point.
#[derive(Clone, Copy)]
struct P {
    t: f32,
    v: f32,
    bend: f32,
}

fn p(t: f32, v: f32, bend: f32) -> P {
    P { t, v, bend }
}

thread_local! {
    /// The picture's drawing room in points (read back from the page), so the curve's lines are laid out in it.
    static PLOT: std::cell::Cell<(f32, f32)> = const { std::cell::Cell::new((1000.0, 1000.0)) };
}
fn vx(t: f32) -> f32 {
    t * PLOT.get().0
}
fn vy(v: f32) -> f32 {
    (1.0 - v) * PLOT.get().1
}

/// The curve as Path commands in a 1000 by 1000 box: each line a quadratic with its handle where the page puts it
/// (mo_app::curve_groups: (0.5 + bend / 2, 0.5 - bend / 2) along the line).
fn path(points: &[P]) -> String {
    let mut s = String::new();
    for (i, a) in points.iter().enumerate() {
        if i == 0 {
            s += &format!("M {:.1} {:.1} ", vx(a.t), vy(a.v));
        }
        if let Some(b) = points.get(i + 1) {
            let (cx, cy) = (0.5 + a.bend / 2.0, 0.5 - a.bend / 2.0);
            let qx = a.t + (b.t - a.t) * cx;
            let qy = a.v + (b.v - a.v) * cy;
            // As short straight lines along the quadratic (Slint's software renderer drew chained Q's out of place).
            let steps = if a.bend.abs() < 0.01 { 1 } else { 32 };
            for k in 1..=steps {
                let u = k as f32 / steps as f32;
                let x = 2.0 * u * (1.0 - u) * qx + u * u * b.t + (1.0 - u) * (1.0 - u) * a.t;
                let y = 2.0 * u * (1.0 - u) * qy + u * u * b.v + (1.0 - u) * (1.0 - u) * a.v;
                s += &format!("L {:.1} {:.1} ", vx(x), vy(y));
            }
        }
    }
    s
}

fn fill(points: &[P]) -> String {
    let (Some(f), Some(l)) = (points.first(), points.last()) else { return String::new() };
    format!("{}L {:.1} {:.1} L {:.1} {:.1} Z", path(points), vx(l.t), vy(0.0), vx(f.t), vy(0.0))
}

/// Value at t, for the bend knob and the read-out: the line's quadratic, sampled.
fn at(points: &[P], t: f32) -> f32 {
    let i = points.iter().rposition(|q| q.t <= t).unwrap_or(0);
    let (a, b) = (points[i], points[(i + 1).min(points.len() - 1)]);
    if b.t <= a.t {
        return b.v;
    }
    let (cx, cy) = (0.5 + a.bend / 2.0, 0.5 - a.bend / 2.0);
    let (qx, qy) = (a.t + (b.t - a.t) * cx, a.v + (b.v - a.v) * cy);
    let mut best = (f32::MAX, a.v);
    for k in 0..=400 {
        let u = k as f32 / 400.0;
        let x = (1.0 - u) * (1.0 - u) * a.t + 2.0 * u * (1.0 - u) * qx + u * u * b.t;
        let y = (1.0 - u) * (1.0 - u) * a.v + 2.0 * u * (1.0 - u) * qy + u * u * b.v;
        if (x - t).abs() < best.0 {
            best = ((x - t).abs(), y);
        }
    }
    best.1
}

/// The part of the curve between two times (whole lines only, plus the lines cut at either end as straight cuts).
fn part(points: &[P], s: f32, e: f32) -> Vec<P> {
    let mut out: Vec<P> = points.iter().copied().filter(|q| q.t >= s - 1e-4 && q.t <= e + 1e-4).collect();
    if out.first().is_none_or(|q| q.t > s + 1e-4) {
        out.insert(0, p(s, at(points, s), 0.0));
    }
    if out.last().is_none_or(|q| q.t < e - 1e-4) {
        out.push(p(e, at(points, e), 0.0));
    }
    out
}

/// "Pump into gate": a bar of four sidechain pumps, then a bar of a 1/16 trance gate.
fn pump_into_gate(bend3: f32) -> Vec<P> {
    let mut pts = Vec::new();
    for k in 0..4 {
        let s = k as f32 * 0.125;
        let bend = if k == 2 { bend3 } else { 0.7 };
        pts.push(p(s, 0.04, bend));
        pts.push(p(s + 0.121, 1.0, 0.0));
    }
    pts.extend(gate(0.5, &[1.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0, 0.7, 0.0, 1.0, 0.0, 0.55, 0.0, 0.3, 0.0]));
    pts
}

/// A gate of steps from `from`, each 1/32 of the picture (1/16 of a bar over two bars).
fn gate(from: f32, levels: &[f32]) -> Vec<P> {
    let w = 0.5 / levels.len() as f32;
    let mut pts = Vec::new();
    for (i, l) in levels.iter().enumerate() {
        let s = from + i as f32 * w;
        pts.push(p(s, *l, 0.0));
        pts.push(p(s + w, *l, 0.0));
    }
    pts
}

fn grid(bars: u32, per_bar: u32) -> Vec<GridLine> {
    let n = bars * per_bar;
    (0..=n)
        .map(|i| GridLine {
            x: i as f32 / n as f32,
            kind: if i % per_bar == 0 {
                0
            } else if i % (per_bar / 4).max(1) == 0 {
                1
            } else {
                2
            },
        })
        .collect()
}

fn ruler(bars: u32) -> Vec<RulerMark> {
    let mut m = Vec::new();
    for b in 0..bars {
        for beat in 0..4 {
            let x = (b * 4 + beat) as f32 / (bars * 4) as f32;
            let text = if beat == 0 { format!("{}", b + 1) } else { format!("{}.{}", b + 1, beat + 1) };
            m.push(RulerMark { x, text: text.into() });
        }
    }
    m
}

/// The dots: a point shows when it's at least 24 points (a target you can grab) from the last one shown, measured on
/// the screen; the rest show only under the mouse, so a dense gate doesn't turn into a row of beads.
fn dots(points: &[P], _width_px: f32, hover: Option<usize>, held: Option<usize>) -> Vec<Pt> {
    let (w, h) = PLOT.get();
    let mut last: Option<(f32, f32)> = None;
    points
        .iter()
        .enumerate()
        .map(|(i, q)| {
            let here = (q.t * w, q.v * h);
            let far = last.is_none_or(|(x, y)| ((here.0 - x).powi(2) + (here.1 - y).powi(2)).sqrt() >= 24.0);
            let state = if Some(i) == held {
                2
            } else if Some(i) == hover {
                1
            } else if far {
                0
            } else {
                -1
            };
            if state >= 0 {
                last = Some(here);
            }
            Pt { x: q.t, y: q.v, state }
        })
        .collect()
}

fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(v))
}

fn hint(parts: &[(&str, &str)]) -> ModelRc<HintPart> {
    model(parts.iter().map(|(k, t)| HintPart { key: (*k).into(), text: (*t).into() }).collect())
}

fn choice(text: &str, count: &str, on: bool) -> FilterChoice {
    FilterChoice { text: text.into(), count: count.into(), on }
}

fn fv(chips: Vec<FilterChoice>) -> FilterView {
    FilterView { chips: model(chips), more: model(vec![]), more_picked: SharedString::new() }
}

fn clip(start: f32, end: f32, name: &str, length: &str) -> Clip {
    Clip {
        start,
        end,
        name: name.into(),
        length: length.into(),
        pinned: false,
        picked: false,
        drop: false,
        hover: false,
    }
}

fn no_hl() -> Highlight {
    Highlight { on: false, start: 0.0, end: 0.0, low: 0.0, high: 1.0, hot: "".into(), held: false }
}

/// The editor picture's width at a window width (rail, margins, filter column, list and card padding taken off).
fn picture_width(window: u32) -> f32 {
    window as f32 - 208.0 - 1.0 - 48.0 - 208.0 - 16.0 - 268.0 - 16.0 - 32.0 - 2.0 - 28.0
}

/// The page every state starts from: Pump into gate picked, edited, nothing highlighted.
fn base(m: &Mock, width: u32) {
    m.set_shape(fv(vec![
        choice("Rise", "31", false),
        choice("Fall", "44", false),
        choice("Pump", "88", false),
        choice("Gate", "52", true),
        choice("Steps", "27", false),
        choice("Other", "170", false),
    ]));
    m.set_rhythm(fv(vec![
        choice("1/4", "61", false),
        choice("1/8", "74", false),
        choice("1/16", "19", true),
        choice("1/8 triplet", "12", false),
        choice("Mixed", "40", false),
    ]));
    m.set_feel(fv(vec![choice("Straight", "301", false), choice("Swung", "36", false), choice("Uneven", "75", false)]));
    let sq = "M 0 90 L 0 10 L 40 10 L 40 90 L 75 90 L 75 10 L 115 10 L 115 90 L 150 90 L 150 10 L 190 10 L 190 90 L 225 90 L 225 10 L 265 10 L 265 90 L 300 90";
    let jaz = "M 0 90 L 0 10 L 40 10 L 40 90 L 75 90 L 75 40 L 115 40 L 115 90 L 150 90 L 150 10 L 190 10 L 190 90 L 225 90 L 225 60 L 265 60 L 265 90 L 300 90";
    let into = "M 0 90 L 0 10 L 20 10 L 20 90 L 30 90 L 30 10 L 50 10 L 50 90 L 120 90 L 120 10 L 170 10 L 170 90 L 230 90 L 230 10 L 300 10";
    let sc = "M 0 90 Q 60 90 74 10 L 76 90 Q 135 90 149 10 L 151 90 Q 210 90 224 10 L 226 90 Q 285 90 299 10";
    m.set_lines(model(vec![
        CurveLine { name: "SC 1".into(), second: "Pump · 1/4 · 1 bar".into(), shape: sc.into(), picked: true, lifted: false },
        CurveLine { name: "Trance gate 3".into(), second: "Gate · 1/16 · 1 bar".into(), shape: sq.into(), picked: false, lifted: false },
        CurveLine { name: "Jazzy Filtering".into(), second: "Gate · 1/16 · 1 bar".into(), shape: jaz.into(), picked: false, lifted: false },
        CurveLine { name: "Into Action".into(), second: "Gate · 1/32 · half speed".into(), shape: into.into(), picked: false, lifted: false },
        CurveLine { name: "Choppy 16".into(), second: "Gate · 1/16 · 1 bar".into(), shape: sq.into(), picked: false, lifted: false },
        CurveLine { name: "Stutter Lift".into(), second: "Gate · 1/16 · 2 bars".into(), shape: jaz.into(), picked: false, lifted: false },
        CurveLine { name: "Half Gate".into(), second: "Gate · 1/16 · 1 bar".into(), shape: sq.into(), picked: false, lifted: false },
        CurveLine { name: "Offbeat Chop".into(), second: "Gate · 1/16 · 1 bar".into(), shape: jaz.into(), picked: false, lifted: false },
        CurveLine { name: "Trance gate 7".into(), second: "Gate · 1/16 · 1 bar".into(), shape: sq.into(), picked: false, lifted: false },
    ]));
    m.set_ruler(model(ruler(2)));
    m.set_grid(model(grid(2, 16)));
    m.set_seams(model(vec![0.5]));
    m.set_swing(model(vec![]));
    m.set_clips(model(vec![clip(0.0, 0.5, "SC 1", "1 bar"), clip(0.5, 1.0, "Trance gate 3", "1 bar")]));
    let pts = pump_into_gate(0.7);
    m.set_curve(path(&pts).into());
    m.set_fill_path(fill(&pts).into());
    m.set_points(model(dots(&pts, picture_width(width), None, None)));
    m.set_hl(no_hl());
    m.set_whole_handles(true);
}

fn capture(window: &MinimalSoftwareWindow, w: u32, h: u32) -> image::RgbImage {
    let mut buf = vec![Rgb8Pixel::default(); (w * h) as usize];
    window.draw_if_needed(|r| {
        r.render(&mut buf, w as usize);
    });
    let raw: Vec<u8> = buf.iter().flat_map(|p| [p.r, p.g, p.b]).collect();
    image::RgbImage::from_raw(w, h, raw).unwrap_or_default()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "shots".into()));
    std::fs::create_dir_all(&out)?;
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(Headless(window.clone())))
        .map_err(|e| format!("couldn't set up drawing: {e}"))?;

    type State = (&'static str, u32, u32, fn(&Mock, u32));
    let states: Vec<State> = vec![
        ("1-point", 1440, 900, s_point),
        ("2-alt-bend", 1440, 900, s_bend),
        ("3-highlight-squash", 1440, 900, s_squash),
        ("4-ctrl-stutter", 1440, 900, s_stutter),
        ("5-drop-swap", 1440, 900, s_drop),
        ("6-randomised", 1440, 900, s_random),
        ("7-highlight-menu", 1440, 900, s_menu),
        ("8-ctrl-paint", 1440, 900, s_paint),
        ("9-nothing-picked", 1440, 900, s_empty),
        ("10-found-in", 1440, 900, s_found),
        ("wide-4-ctrl-stutter", 3440, 1369, s_stutter),
        ("wide-6-randomised", 3440, 1369, s_random),
    ];
    for (name, w, h, f) in states {
        window.set_size(PhysicalSize::new(w, h));
        let m = Mock::new()?;
        m.set_curves_colour(slint::Color::from_rgb_u8(0x35, 0xD0, 0xBA));
        m.set_wide(w >= 2000);
        m.show()?;
        // A first drawing lays the page out; then the curve is laid out in the picture's own points.
        base(&m, w);
        f(&m, w);
        let _ = capture(&window, w, h);
        PLOT.set((m.get_plot_width(), m.get_plot_height()));
        base(&m, w);
        f(&m, w);
        window.request_redraw();
        let img = capture(&window, w, h);
        img.save(out.join(format!("curve-editor-{name}.png")))?;
        m.hide()?;
        println!("{name}");
    }
    Ok(())
}

/// A point under the mouse in the first bar.
fn s_point(m: &Mock, w: u32) {
    let pts = pump_into_gate(0.7);
    m.set_points(model(dots(&pts, picture_width(w), Some(3), None)));
    m.set_mouse_x(pts[3].t + 0.002);
    m.set_mouse_y(pts[3].v - 0.01);
    m.set_cursor("grab".into());
    m.set_hint_title("Point".into());
    m.set_hint(hint(&[("", "drag to move"), ("", "double-click removes it"), ("", "right-click types a value")]));
}

/// Alt held over the third pump's rise, mid-drag: the old shape faint, the bend knob and a read-out.
fn s_bend(m: &Mock, w: u32) {
    let old = pump_into_gate(0.7);
    let pts = pump_into_gate(-0.6);
    m.set_curve(path(&pts).into());
    m.set_fill_path(fill(&pts).into());
    m.set_ghost(path(&old[4..6]).into());
    m.set_hot_line(path(&pts[4..6]).into());
    m.set_points(model(dots(&pts, picture_width(w), None, None)));
    m.set_bend_knob(true);
    m.set_knob_x(0.3125);
    m.set_knob_y(at(&pts, 0.3125));
    m.set_mouse_x(0.3125);
    m.set_mouse_y(at(&pts, 0.3125));
    m.set_cursor("ns".into());
    m.set_readout("bend 60% early".into());
    m.set_readout_x(0.3125);
    m.set_readout_y(at(&pts, 0.3125));
    m.set_alt(true);
    m.set_hint_title("Bend this line".into());
    m.set_hint(hint(&[("", "up bends early, down late"), ("", "Alt-double-click: straight")]));
}

/// A highlight over the gate's first half, its top handle held and dragged down to 60%.
fn s_squash(m: &Mock, w: u32) {
    let old = pump_into_gate(0.7);
    let mut seen_end = false;
    let pts: Vec<P> = old
        .iter()
        .map(|q| {
            let inside = q.t >= 0.5 - 1e-4 && q.t < 0.75 - 1e-4;
            let end = (q.t - 0.75).abs() < 1e-4 && !seen_end;
            if end {
                seen_end = true;
            }
            if inside || end { p(q.t, q.v * 0.6, q.bend) } else { *q }
        })
        .collect();
    let pts = fix_join(pts);
    m.set_curve(path(&pts).into());
    m.set_fill_path(fill(&pts).into());
    m.set_curve_lit(path(&part(&pts, 0.5, 0.75)).into());
    m.set_ghost(path(&part(&old, 0.5, 0.75)).into());
    m.set_points(model(dots(&pts, picture_width(w), None, None)));
    m.set_whole_handles(false);
    m.set_hl(Highlight { on: true, start: 0.5, end: 0.75, low: 0.0, high: 0.6, hot: "top".into(), held: true });
    m.set_mouse_x(0.625);
    m.set_mouse_y(0.6);
    m.set_cursor("ns".into());
    m.set_readout("height 60%".into());
    m.set_readout_x(0.625);
    m.set_readout_y(0.6);
    m.set_hint_title("Top of the highlight".into());
    m.set_hint(hint(&[("", "drag down squashes"), ("", "through the bottom flips it")]));
}

/// The jump at a clip's seam stays a jump: the point before the part keeps its height.
fn fix_join(pts: Vec<P>) -> Vec<P> {
    pts
}

/// Two gate steps highlighted, their right side Ctrl-dragged out to fill half a bar: a stutter, repeated 4 times.
fn s_stutter(m: &Mock, w: u32) {
    let old = pump_into_gate(0.7);
    let mut pts: Vec<P> = old.iter().copied().filter(|q| q.t < 0.5 - 1e-4 || q.t > 0.75 + 1e-4).collect();
    // The step after the part starts where it did.
    let after = pts.iter().position(|q| q.t > 0.75).unwrap_or(pts.len());
    pts.insert(after, p(0.75, 0.7, 0.0));
    let rep = gate(0.5, &[1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0]);
    // gate() spreads steps over half the picture; take them at 1/32 each over a quarter.
    let rep: Vec<P> = rep.iter().map(|q| p(0.5 + (q.t - 0.5) * 0.5, q.v, q.bend)).collect();
    let at_split = pts.iter().position(|q| q.t > 0.5).unwrap_or(pts.len());
    for (k, q) in rep.into_iter().enumerate() {
        pts.insert(at_split + k, q);
    }
    m.set_curve(path(&pts).into());
    m.set_fill_path(fill(&pts).into());
    m.set_curve_lit(path(&part(&pts, 0.5, 0.75)).into());
    m.set_ghost(path(&part(&old, 0.5625, 0.75)).into());
    m.set_points(model(dots(&pts, picture_width(w), None, None)));
    m.set_whole_handles(false);
    m.set_hl(Highlight { on: true, start: 0.5, end: 0.75, low: 0.0, high: 1.0, hot: "right".into(), held: true });
    m.set_mouse_x(0.75);
    m.set_mouse_y(0.5);
    m.set_cursor("ew".into());
    m.set_readout("repeat ×4 · 1/16".into());
    m.set_readout_x(0.75);
    m.set_readout_y(0.5);
    m.set_ctrl(true);
    m.set_hint_title("Repeat the part".into());
    m.set_hint(hint(&[("", "drag out to repeat it"), ("", "a short part makes a stutter")]));
    let mut c = vec![clip(0.0, 0.5, "SC 1", "1 bar"), clip(0.5, 1.0, "Trance gate 3", "1 bar")];
    c[1].hover = false;
    m.set_clips(model(c));
}

/// A curve carried from the list over the second clip: it would swap in, fitted; the picture shows it already.
fn s_drop(m: &Mock, w: u32) {
    let old = pump_into_gate(0.7);
    let mut pts: Vec<P> = old.iter().copied().filter(|q| q.t < 0.5 - 1e-4).collect();
    pts.extend(gate(0.5, &[1.0, 0.0, 0.6, 0.0, 1.0, 0.0, 0.4, 0.0, 1.0, 0.0, 0.6, 0.0, 1.0, 1.0, 0.2, 0.0]));
    m.set_curve(path(&pts).into());
    m.set_fill_path(fill(&pts).into());
    m.set_curve_lit(path(&part(&pts, 0.5, 1.0)).into());
    m.set_ghost(path(&part(&old, 0.5, 1.0)).into());
    m.set_points(model(dots(&pts, picture_width(w), None, None)));
    let mut c = vec![clip(0.0, 0.5, "SC 1", "1 bar"), clip(0.5, 1.0, "Jazzy Filtering", "1 bar")];
    c[1].drop = true;
    m.set_clips(model(c));
    m.set_whole_handles(false);
    m.set_readout("swap in · fitted to 1 bar".into());
    m.set_readout_x(0.62);
    m.set_readout_y(1.02);
    m.set_carried("Jazzy Filtering".into());
    m.set_carried_x(1000.0);
    m.set_carried_y(222.0);
    let mut lines: Vec<CurveLine> = (0..m.get_lines().row_count()).filter_map(|i| m.get_lines().row_data(i)).collect();
    lines[2].lifted = true;
    m.set_lines(model(lines));
    m.set_hint_title("Drop on a clip".into());
    m.set_hint(hint(&[("", "swaps it in, fitted"), ("", "between two clips adds it")]));
}

/// After Randomise at 2 bars: four parts from the gates shown, the second pinned, the last highlighted to roll again.
fn s_random(m: &Mock, w: u32) {
    let mut pts = gate(0.0, &[1.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0]);
    pts = pts.iter().map(|q| p(q.t * 0.5, q.v, 0.0)).collect();
    let b: Vec<P> = gate(0.0, &[1.0, 0.0, 0.6, 0.0, 1.0, 0.0, 0.4, 0.0]).iter().map(|q| p(0.25 + q.t * 0.5, q.v, 0.0)).collect();
    let c: Vec<P> = gate(0.0, &[1.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0]).iter().map(|q| p(0.5 + q.t * 0.5, q.v * 0.7, 0.0)).collect();
    let d: Vec<P> = gate(0.0, &[1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0])
        .iter()
        .map(|q| p(0.75 + q.t * 0.5, q.v * (1.0 - (q.t * 2.0) * 0.8), 0.0))
        .collect();
    pts.extend(b);
    pts.extend(c);
    pts.extend(d);
    m.set_name("Random gate".into());
    let mut lines: Vec<CurveLine> = (0..m.get_lines().row_count()).filter_map(|i| m.get_lines().row_data(i)).collect();
    lines[0].picked = false;
    m.set_lines(model(lines));
    m.set_state_words("new, not saved".into());
    m.set_curve(path(&pts).into());
    m.set_fill_path(fill(&pts).into());
    m.set_curve_lit(path(&part(&pts, 0.75, 1.0)).into());
    m.set_points(model(dots(&pts, picture_width(w), None, None)));
    let mut cl = vec![
        clip(0.0, 0.25, "Trance gate 3", "1/2 bar"),
        clip(0.25, 0.5, "Jazzy Filtering", "1/2 bar"),
        clip(0.5, 0.75, "Trance gate 3", "1/2 bar · 70%"),
        clip(0.75, 1.0, "Stutter Lift", "1/2 bar · ×2 · fade"),
    ];
    cl[1].pinned = true;
    cl[3].picked = true;
    cl[1].hover = true;
    m.set_clips(model(cl));
    m.set_seams(model(vec![0.25, 0.5, 0.75]));
    m.set_whole_handles(false);
    m.set_hl(Highlight { on: true, start: 0.75, end: 1.0, low: 0.0, high: 1.0, hot: "".into(), held: false });
    m.set_length_pick(1);
    m.set_wild(0.55);
    m.set_wild_word("Halfway".into());
    m.set_hint_title("Randomise".into());
    m.set_hint(hint(&[("Ctrl+R", "rolls the highlighted part again")]));
}

/// Right-click on a highlight: the menu that holds Make into steps, and the gestures again with their keys.
fn s_menu(m: &Mock, w: u32) {
    let pts = pump_into_gate(0.7);
    m.set_curve_lit(path(&part(&pts, 0.0, 0.25)).into());
    m.set_points(model(dots(&pts, picture_width(w), None, None)));
    m.set_whole_handles(false);
    m.set_hl(Highlight { on: true, start: 0.0, end: 0.25, low: 0.0, high: 1.0, hot: "".into(), held: false });
    let mut c = vec![clip(0.0, 0.5, "SC 1", "1 bar"), clip(0.5, 1.0, "Trance gate 3", "1 bar")];
    c[0].hover = true;
    m.set_clips(model(c));
    m.set_menu_open(true);
    m.set_menu_x(200.0);
    m.set_menu_y(300.0);
    m.set_hint_title("Highlight".into());
    m.set_hint(hint(&[("", "right-click for more"), ("Esc", "drops it")]));
}

/// Ctrl held over empty space: painting steps on the grid, one per square.
fn s_paint(m: &Mock, w: u32) {
    let old = pump_into_gate(0.7);
    let levels = [0.9, 0.2, 0.7, 0.35, 0.5];
    let end = 0.75 + levels.len() as f32 / 32.0;
    let mut pts: Vec<P> = old.iter().copied().filter(|q| q.t < 0.75 - 1e-4).collect();
    pts.push(p(0.75, 1.0, 0.0));
    for (k, v) in levels.iter().enumerate() {
        let s = 0.75 + k as f32 / 32.0;
        pts.push(p(s, *v, 0.0));
        pts.push(p(s + 1.0 / 32.0, *v, 0.0));
    }
    pts.push(p(end, at(&old, end + 1e-3), 0.0));
    pts.extend(old.iter().copied().filter(|q| q.t > end + 1e-4));
    m.set_curve(path(&pts).into());
    m.set_fill_path(fill(&pts).into());
    m.set_ghost(path(&part(&old, 0.75, end)).into());
    let mut blocks = String::new();
    for (k, v) in levels.iter().enumerate() {
        let s = 0.75 + k as f32 / 32.0;
        let e = s + 1.0 / 32.0;
        blocks += &format!(
            "M {:.1} {:.1} L {:.1} {:.1} L {:.1} {:.1} L {:.1} {:.1} Z ",
            vx(s),
            vy(0.0),
            vx(s),
            vy(*v),
            vx(e),
            vy(*v),
            vx(e),
            vy(0.0)
        );
    }
    m.set_paint(blocks.into());
    m.set_points(model(dots(&pts, picture_width(w), None, None)));
    m.set_whole_handles(false);
    m.set_mouse_x(0.75 + 4.5 / 32.0);
    m.set_mouse_y(0.5);
    m.set_cursor("cross".into());
    m.set_readout("step 5 · 50%".into());
    m.set_readout_x(0.75 + 4.5 / 32.0);
    m.set_readout_y(0.5);
    m.set_ctrl(true);
    m.set_hint_title("Paint steps".into());
    m.set_hint(hint(&[("", "one step per grid square"), ("Shift", "freehand")]));
}

/// Not edited: the picked curve's own buttons work, and "In 2 songs" is open.
fn s_found(m: &Mock, w: u32) {
    m.set_state_words("".into());
    let pts = pump_into_gate(0.7);
    m.set_points(model(dots(&pts, picture_width(w), None, None)));
    m.set_found_open(true);
    m.set_hint_title("In 2 songs".into());
    m.set_hint(hint(&[("", "pick one to show it on Songs")]));
}

fn s_empty(m: &Mock, _w: u32) {
    m.set_facts("".into());
    m.set_name("".into());
    m.set_state_words("".into());
    m.set_curve("".into());
    m.set_fill_path("".into());
    m.set_points(model(vec![]));
    m.set_clips(model(vec![]));
    m.set_seams(model(vec![]));
    m.set_whole_handles(false);
    let mut lines: Vec<CurveLine> = (0..m.get_lines().row_count()).filter_map(|i| m.get_lines().row_data(i)).collect();
    lines[0].picked = false;
    m.set_lines(model(lines));
    m.set_empty_title("No curve picked".into());
    m.set_hint_title("".into());
    m.set_hint(hint(&[]));
}

use slint::Model;
