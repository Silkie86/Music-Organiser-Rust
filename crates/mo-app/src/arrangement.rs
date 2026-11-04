//! The arrangement at the top of a song's Overview: the project drawn as its DAW's arrangement shows it (spec section
//! 3, Songs). Track names down the left, bars along the top, and each clip in its place with its notes, its audio or
//! its automation curve inside. The whole song fits the width at first; the mouse wheel scrolls, Ctrl + wheel zooms
//! about the mouse, dragging moves around and a double-click fits it all in (again to go back). Ported from the C#
//! app's `ArrangementView`.
//!
//! [`View`] keeps where the view is and answers the mouse; [`View::frame`] works out what to draw, away from the window's
//! thread: the shapes, as a [`Scene`] the graphics card draws (or the processor, where there's no card: see
//! [`crate::shapes`]), and the words as labels for the window to lay on top, so they use the app's own fonts.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mo_formats::arrangement::{Arrangement, AudioPart, LaneKind, PreviewClip, PreviewLane};

use mo_library::songs::thousands;

use crate::shapes::{Colour, Scene};
use crate::themes::Look;

/// A project's arrangement views, read and with its audio files looked for: what [`read`] gives.
#[derive(Clone, Debug, Default)]
pub struct Preview {
    pub views: Vec<Arc<Arrangement>>,
    /// The audio files found on this computer, by the path the project saved.
    pub found: Arc<HashMap<String, PathBuf>>,
}

/// An audio file's sound boiled down for drawing its waveform: the loudest a channel gets in each slice of the file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Peaks {
    /// How long the whole file is, in seconds (it may be longer than what was read).
    pub file_seconds: f64,
    /// How many seconds each value covers.
    pub step: f64,
    /// The loudest (0 to 1) in each slice, from the file's start.
    pub values: Vec<f32>,
}

/// Each slice of a waveform: 5 ms, so even a clip zoomed far in looks smooth, and a minute is 12,000 numbers.
const PEAK_STEP: f64 = 0.005;

impl Peaks {
    /// Boils down a file's samples (`channels`, at `sample_rate` a second).
    pub fn of(sample_rate: u32, channels: &[Vec<f32>], file_seconds: f64) -> Peaks {
        let per = ((f64::from(sample_rate) * PEAK_STEP).round() as usize).max(1);
        let frames = channels.iter().map(Vec::len).max().unwrap_or(0);
        let values = (0..frames.div_ceil(per))
            .map(|i| {
                let (a, b) = (i * per, ((i + 1) * per).min(frames));
                channels
                    .iter()
                    .flat_map(|c| c.get(a..b.min(c.len())).unwrap_or(&[]))
                    .fold(0.0f32, |m, v| m.max(v.abs()))
                    .min(1.0)
            })
            .collect();
        Peaks { file_seconds, step: per as f64 / f64::from(sample_rate.max(1)), values }
    }

    /// The loudest from `from` to `to` seconds into the file, or none past what was read.
    pub fn loudest(&self, from: f64, to: f64) -> Option<f32> {
        let (from, to) = (from.min(to), from.max(to));
        let first = (from / self.step).floor().max(0.0) as usize;
        let last = ((to / self.step).ceil() as usize).max(first + 1).min(self.values.len());
        (first < last).then(|| self.values[first..last].iter().fold(0.0f32, |m, &v| m.max(v)))
    }
}

/// Reads a project's arrangement and looks for its audio files the way the Samples tab does (rule 13: the project's
/// folders first, then the DAW's own, then the Sounds library at `sounds_db` when given). Reads the disk, so never on the window's thread. An error is the message to
/// show instead.
pub fn read(project: &Path, sounds_db: Option<&Path>) -> Result<Preview, String> {
    // A reader that trips on a damaged save (overflow checks stay on) says so here, as the library's scan does,
    // instead of ending the page's reader with the arrangement never shown.
    let views = std::panic::catch_unwind(|| mo_formats::arrangement::read(project))
        .map_err(|_| "Couldn't read the arrangement: the save holds something the reader didn't expect.".to_owned())?
        .map_err(|e| format!("Couldn't read the arrangement: {e}."))?;
    let locator = crate::sound_library::locator(sounds_db);
    let project_path = project.to_string_lossy();
    let mut found = HashMap::new();
    let mut looked = HashSet::new();
    let mut views: Vec<Arrangement> = views;
    for a in &mut views {
        let mut missing = 0;
        for part in a.lanes.iter().flat_map(|l| &l.clips).flat_map(|c| &c.audio) {
            if looked.insert(part.path.clone()) {
                if let Some(f) = locator.locate(&part.path, &project_path) {
                    found.insert(part.path.clone(), PathBuf::from(f));
                } else if locator.found(&part.path, &project_path) {
                    // The DAW's own content ("%FLStudioFactoryData%…"): there, though not at a path of its own yet.
                    found.insert(part.path.clone(), PathBuf::from(&part.path));
                }
            }
            if !found.contains_key(&part.path) {
                missing += 1;
            }
        }
        if missing > 0 {
            a.notes.push(if missing == 1 {
                "1 audio clip's file isn't on this computer, so it's drawn without its sound.".to_owned()
            } else {
                format!("{missing} audio clips' files aren't on this computer, so they're drawn without their sound.")
            });
        }
    }
    Ok(Preview { views: views.into_iter().map(Arc::new).collect(), found: Arc::new(found) })
}

/// "140 BPM · 32 bars · 0:55 · 6 tracks · 18 clips · 412 notes".
pub fn summary(a: &Arrangement) -> String {
    let mut parts = vec![format!("{} BPM", trim(a.tempo))];
    let length = a.length();
    if length > 0.0 {
        let bars = (a.bars() - 1e-6).ceil().max(0.0) as u64;
        parts.push(count(bars, "bar"));
        let seconds = a.seconds().round() as u64;
        parts.push(if seconds >= 3600 {
            format!("{}:{:02}:{:02}", seconds / 3600, seconds / 60 % 60, seconds % 60)
        } else {
            format!("{}:{:02}", seconds / 60, seconds % 60)
        });
    }
    parts.push(count(a.lanes.iter().filter(|l| l.kind != LaneKind::Group).count() as u64, "track"));
    parts.push(count(a.clip_count() as u64, "clip"));
    let notes = a.note_count();
    if notes > 0 {
        parts.push(count(notes as u64, "note"));
    }
    parts.join(" · ")
}

/// A tempo as it's written: up to two places, no trailing zeros.
fn trim(v: f64) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// "1 bar", "1,204 notes".
fn count(n: u64, what: &str) -> String {
    format!("{} {what}{}", thousands(usize::try_from(n).unwrap_or(usize::MAX)), if n == 1 { "" } else { "s" })
}

/// The colours the drawing uses, from the app's look.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub surface: u32,
    pub raised: u32,
    pub stroke: u32,
    pub hover: u32,
    pub muted: u32,
    pub faint: u32,
    pub accent: u32,
    /// Markers and automation.
    pub warning: u32,
    /// Audio.
    pub ok: u32,
    /// A light theme: notes and curves are drawn darker than their clip, not lighter, so they stand out.
    pub light: bool,
}

impl Palette {
    pub fn from_look(look: &Look) -> Palette {
        let t = &look.theme;
        Palette {
            surface: t.surface,
            raised: t.surface_raised,
            stroke: t.stroke,
            hover: t.hover,
            muted: t.muted,
            faint: t.faint,
            accent: look.accent,
            // The same in every theme, as in the window's Theme.
            warning: 0xF2B33D,
            ok: 0x4ACB8B,
            light: t.light,
        }
    }
}

/// Where a label goes: the strip of track names, the timeline under the ruler, or the ruler. Each is clipped to its
/// own area; positions are from the view's top left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Area {
    Header,
    Timeline,
    Ruler,
}

/// How a label looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    /// A track's name; a group's is bolder.
    Lane { group: bool, muted: bool, small: bool },
    /// What plays a track, under its name.
    Detail,
    /// The view's name, in capitals, at the ruler's left.
    Head,
    /// A bar number.
    Bar,
    /// A marker's name, on its flag.
    Marker,
    /// A clip's name, on its title bar.
    Clip,
    /// "muted", in a clip's corner, and "file not on this computer".
    Small,
}

/// Words to lay over the picture: `width` is the room there is (the window cuts them short), `height` the line they're
/// centred in.
#[derive(Clone, Debug, PartialEq)]
pub struct Label {
    pub area: Area,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub text: String,
    pub style: Style,
}

/// What's under the mouse: where it is, and "Bar 3.2 · Bass · 12 notes".
#[derive(Clone, Debug, PartialEq)]
pub struct Hover {
    pub x: f64,
    pub y: f64,
    pub text: String,
}

/// A view worked out for drawing: its shapes (in points, for the graphics card or [`Scene::paint`]) and the words to lay
/// on them.
#[derive(Clone, Debug)]
pub struct Frame {
    pub scene: Scene,
    pub labels: Vec<Label>,
    pub hover: Option<Hover>,
    pub header_width: f64,
    pub ruler_height: f64,
}

/// The most clip names laid on the picture at once: past that, they're too small to read anyway.
const MOST_CLIP_LABELS: usize = 600;
const SCROLLBAR: f64 = 5.0;

/// Where the view is, kept on the window's thread and copied to the drawing thread for each frame.
#[derive(Clone, Debug)]
pub struct View {
    arrangement: Arc<Arrangement>,
    found: Arc<HashMap<String, PathBuf>>,
    /// The waveforms of the audio files read so far, by the path the project saved.
    peaks: Arc<HashMap<String, Arc<Peaks>>>,
    pub width: f64,
    pub height: f64,
    /// Pixels per beat; 0 fits the song to the width.
    zoom: f64,
    /// The beat at the timeline's left edge, and how far down the lanes are scrolled.
    left: f64,
    top: f64,
    /// After a double-click every lane is squeezed to fit the height, so the whole project is on screen.
    fit_all: bool,
    hover: Option<(f64, f64)>,
    drag: Option<(f64, f64, f64, f64)>,
    song_beats: f64,
    content_height: f64,
    /// Each lane's lowest and highest note, so the same note sits at the same height in every clip.
    keys: Arc<Vec<(u8, u8)>>,
}

fn lane_height(lane: &PreviewLane) -> f64 {
    match lane.kind {
        LaneKind::Group => 26.0,
        LaneKind::Automation => 32.0,
        _ => 46.0,
    }
}

impl View {
    pub fn new(arrangement: Arc<Arrangement>, found: Arc<HashMap<String, PathBuf>>) -> View {
        // Never past the readers' longest song (mo_formats::arrangement::MOST_BEATS), so the bar numbers can count it.
        let length = arrangement.length();
        let song_beats = (arrangement.beats_per_bar.max(1) as f64).max(length * 1.02);
        let content_height = arrangement.lanes.iter().map(lane_height).sum();
        let keys = arrangement
            .lanes
            .iter()
            .map(|l| {
                let mut keys = l.clips.iter().flat_map(|c| &c.notes).map(|n| n.key);
                let first = keys.next();
                first.map_or((60, 60), |k| keys.fold((k, k), |(lo, hi), k| (lo.min(k), hi.max(k))))
            })
            .collect();
        View {
            arrangement,
            found,
            peaks: Arc::default(),
            width: 0.0,
            height: 0.0,
            zoom: 0.0,
            left: 0.0,
            top: 0.0,
            fit_all: false,
            hover: None,
            drag: None,
            song_beats,
            content_height,
            keys: Arc::new(keys),
        }
    }

    pub fn arrangement(&self) -> &Arc<Arrangement> {
        &self.arrangement
    }

    /// The ruler: bar numbers, and a second row for markers when the song has any.
    pub fn ruler_height(&self) -> f64 {
        if self.arrangement.markers.is_empty() { 24.0 } else { 42.0 }
    }

    pub fn header_width(&self) -> f64 {
        if self.width < 520.0 { 118.0 } else { 172.0 }
    }

    fn timeline_width(&self) -> f64 {
        (self.width - self.header_width() - 8.0).max(1.0)
    }

    fn fit_zoom(&self) -> f64 {
        self.timeline_width() / self.song_beats
    }

    fn zoom(&self) -> f64 {
        if self.zoom > 0.0 { self.zoom } else { self.fit_zoom() }
    }

    fn lane_scale(&self) -> f64 {
        if self.fit_all && self.content_height > 0.0 {
            ((self.height - self.ruler_height()) / self.content_height).clamp(0.0, 1.0)
        } else {
            1.0
        }
    }

    /// Draws the audio clips' waveforms from these (the audio files are read after the arrangement, so it shows first).
    pub fn set_peaks(&mut self, peaks: Arc<HashMap<String, Arc<Peaks>>>) {
        self.peaks = peaks;
    }

    fn x(&self, beat: f64) -> f64 {
        self.header_width() + (beat - self.left) * self.zoom()
    }

    fn beat(&self, x: f64) -> f64 {
        self.left + (x - self.header_width()) / self.zoom()
    }

    fn clamp(&mut self) {
        let visible = self.timeline_width() / self.zoom();
        self.left = self.left.clamp(0.0, (self.song_beats - visible).max(0.0));
        let lanes = self.height - self.ruler_height();
        self.top = self.top.clamp(0.0, (self.content_height * self.lane_scale() - lanes).max(0.0));
    }

    pub fn resize(&mut self, width: f64, height: f64) {
        (self.width, self.height) = (width.max(0.0), height.max(0.0));
        self.clamp();
    }

    /// The mouse wheel, by notches (positive away from you): scrolls up and down, sideways with Shift (or a sideways
    /// wheel), and zooms about the mouse with Ctrl.
    pub fn wheel(&mut self, x: f64, across: f64, down: f64, ctrl: bool, shift: bool) {
        let hx = self.header_width();
        if ctrl {
            let at = x.max(hx);
            let beat = self.beat(at);
            let fit = self.fit_zoom();
            let zoom = (self.zoom() * 1.25f64.powf(down)).clamp(fit, fit.max(400.0));
            self.left = beat - (at - hx) / zoom;
            // All the way out is "fit", so the whole song keeps fitting when the panel is resized.
            self.zoom = if zoom <= fit * 1.000_001 { 0.0 } else { zoom };
        } else if shift || across.abs() > down.abs() {
            self.left -= (if across != 0.0 { across } else { down }) * 60.0 / self.zoom();
        } else {
            self.top -= down * 40.0;
        }
        self.clamp();
    }

    pub fn press(&mut self, x: f64, y: f64) {
        self.drag = Some((x, y, self.left, self.top));
    }

    /// The whole song across and every track down; again goes back to full-height tracks.
    pub fn double_click(&mut self) {
        self.fit_all = !(self.fit_all && self.zoom == 0.0);
        self.zoom = 0.0;
        (self.left, self.top) = (0.0, 0.0);
        self.drag = None;
    }

    pub fn moved(&mut self, x: f64, y: f64) {
        self.hover = Some((x, y));
        if let Some((fx, fy, left, top)) = self.drag {
            self.left = left - (x - fx) / self.zoom();
            self.top = top - (y - fy);
            self.clamp();
        }
    }

    pub fn release(&mut self) {
        self.drag = None;
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    pub fn leave(&mut self) {
        self.hover = None;
    }

    /// Where a clip is drawn in a lane whose top is at `y` and `lh` high.
    fn clip_box(&self, clip: &PreviewClip, y: f64, lh: f64) -> Box2 {
        let (x0, x1) = (self.x(clip.start), self.x(clip.end()));
        let pad = (lh * 0.1).min(3.0);
        Box2::new(x0 + 0.5, y + pad, (x1 - x0 - 1.0).max(2.0), (lh - 2.0 * pad - 1.0).max(1.0))
    }

    /// What's under the mouse: the bar and beat, and the clip there. Cheap (no drawing), so the window can ask on
    /// every mouse move; nothing while dragging or off the timeline.
    pub fn hover(&self) -> Option<Hover> {
        let (px, py) = self.hover?;
        let (hx, ruler) = (self.header_width(), self.ruler_height());
        if self.drag.is_some() || px <= hx || px >= self.width || py >= self.height || self.width < 60.0 {
            return None;
        }
        let a = &self.arrangement;
        let beat = self.beat(px).max(0.0);
        let bpb = a.beats_per_bar.max(1) as f64;
        let mut text = format!("Bar {}.{}", ((beat / bpb) as u64).saturating_add(1), (beat % bpb) as u64 + 1);
        if py >= ruler {
            let scale = self.lane_scale();
            let mut y = ruler - self.top;
            for lane in &a.lanes {
                let lh = lane_height(lane) * scale;
                if py >= y && py < y + lh {
                    if let Some(clip) = lane.clips.iter().rev().find(|c| self.clip_box(c, y, lh).contains(px, py)) {
                        text.push_str("  ·  ");
                        text.push_str(&describe(lane, clip, &self.found));
                    }
                    break;
                }
                y += lh;
            }
        }
        Some(Hover { x: px, y: py, text })
    }

    /// How many bars apart the ruler's numbers are, so they're at least 44 points apart.
    fn bar_step(&self) -> u64 {
        let bar = self.zoom() * self.arrangement.beats_per_bar.max(1) as f64;
        let mut step = 1;
        while (step as f64) * bar < 44.0 && step < 1 << 40 {
            step *= 2;
        }
        step
    }

    /// Each bar with a number on the ruler, with where it is.
    fn bars(&self, step: u64) -> Vec<(u64, f64)> {
        let bpb = self.arrangement.beats_per_bar.max(1) as f64;
        let zoom = self.zoom();
        if !(zoom > 0.0 && zoom.is_finite()) {
            return Vec::new();
        }
        let first = ((self.left / bpb / step as f64).floor().max(0.0) as u64).saturating_mul(step);
        let mut out = Vec::new();
        let mut bar = first;
        // At most so many turns, whatever is pushed: far to the right the numbers stop moving.
        for _ in 0..8192 {
            let x = self.x(bar as f64 * bpb);
            if x > self.width || out.len() > 4096 {
                break;
            }
            if x >= self.header_width() {
                out.push((bar, x));
            }
            let Some(next) = bar.checked_add(step) else { break };
            bar = next;
        }
        out
    }

    /// Works out what to draw for the view at `scale` pixels to a point (a waveform has a column for each pixel). Away
    /// from the window's thread: a big song has millions of notes.
    pub fn frame(&self, palette: &Palette, scale: f64) -> Frame {
        let mut d = Draw::new(self, palette, scale);
        d.draw();
        Frame {
            scene: d.scene,
            labels: d.labels,
            hover: self.hover(),
            header_width: self.header_width(),
            ruler_height: self.ruler_height(),
        }
    }
}

/// A rectangle in points.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Box2 {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

impl Box2 {
    fn new(x: f64, y: f64, w: f64, h: f64) -> Box2 {
        Box2 { x, y, w, h }
    }

    fn right(&self) -> f64 {
        self.x + self.w
    }

    fn bottom(&self) -> f64 {
        self.y + self.h
    }

    fn intersect(&self, o: &Box2) -> Option<Box2> {
        let (x, y) = (self.x.max(o.x), self.y.max(o.y));
        let (r, b) = (self.right().min(o.right()), self.bottom().min(o.bottom()));
        (r > x && b > y).then(|| Box2::new(x, y, r - x, b - y))
    }

    fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }
}

/// Crisp rectangles of one colour, as few as fill the same pixels: ones in the same row of pixels that touch or overlap
/// there become one. Fitted to the view, a dense clip has many notes to a pixel, and a huge song millions; this keeps
/// them to about one a run of pixels (what the view has room for), so drawing them stays quick on the card and the
/// processor alike, and a see-through clip's notes don't build up where they overlap.
struct Runs {
    scale: f32,
    /// Each run, and its left and right in pixels.
    runs: Vec<(Box2, f32, f32)>,
    /// For each row of pixels (its top and bottom), the run added to it last.
    last: HashMap<(i32, i32), usize>,
}

impl Runs {
    fn new(scale: f64) -> Runs {
        Runs { scale: scale as f32, runs: Vec::new(), last: HashMap::new() }
    }

    /// Adds a rectangle: to the run last added in its row of pixels when the two touch or overlap there, else as a
    /// run of its own. One that fills no pixel is left out, as either painter would leave it.
    fn add(&mut self, b: Box2) {
        let (x, y, w, h) = (b.x as f32, b.y as f32, b.w as f32, b.h as f32);
        let Some([left, top, right, bottom]) = crate::shapes::crisp([x, y, x + w, y + h], self.scale) else {
            return;
        };
        let row = (top as i32, bottom as i32);
        if let Some(&i) = self.last.get(&row)
            && let Some((run, l, r)) = self.runs.get_mut(i)
            && left <= *r
            && right >= *l
        {
            let (from, to) = (run.x.min(b.x), run.right().max(b.right()));
            *run = Box2::new(from, run.y, to - from, run.h);
            (*l, *r) = (l.min(left), r.max(right));
            return;
        }
        self.last.insert(row, self.runs.len());
        self.runs.push((b, left, right));
    }

    fn boxes(self) -> impl Iterator<Item = Box2> {
        self.runs.into_iter().map(|(b, ..)| b)
    }
}

/// Moves a colour `amount` of the way to black.
fn darken(rgb: u32, amount: f64) -> u32 {
    let ch = |shift: u32| (((rgb >> shift) & 0xFF) as f64 * (1.0 - amount)).round().clamp(0.0, 255.0) as u32;
    ch(16) << 16 | ch(8) << 8 | ch(0)
}

/// `rgb` moved `amount` of the way to white.
fn lighten(rgb: u32, amount: f64) -> u32 {
    let ch = |shift: u32| {
        let c = ((rgb >> shift) & 0xFF) as f64;
        ((c + (255.0 - c) * amount).round() as u32).min(255)
    };
    ch(16) << 16 | ch(8) << 8 | ch(0)
}

/// Works out a view's shapes and words.
struct Draw<'a> {
    v: &'a View,
    p: &'a Palette,
    /// Pixels to a point.
    scale: f64,
    scene: Scene,
    labels: Vec<Label>,
    clip_labels: usize,
}

impl<'a> Draw<'a> {
    fn new(v: &'a View, p: &'a Palette, scale: f64) -> Draw<'a> {
        let scale = crate::shapes::usable_scale(scale);
        Draw { v, p, scale, scene: Scene::new(v.width, v.height, p.surface), labels: Vec::new(), clip_labels: 0 }
    }

    fn rect(&mut self, r: Box2, c: Colour) {
        self.scene.rect((r.x, r.y, r.w, r.h), c);
    }

    /// A rectangle with rounded corners, filled and (when `edge` is some) outlined.
    fn rounded(&mut self, r: Box2, radius: f64, fill: Colour, edge: Option<Colour>) {
        self.scene.rounded((r.x, r.y, r.w, r.h), radius, fill, edge);
    }

    fn label(&mut self, area: Area, r: Box2, text: impl Into<String>, style: Style) {
        self.labels.push(Label { area, x: r.x, y: r.y, width: r.w.max(0.0), height: r.h, text: text.into(), style });
    }

    fn draw(&mut self) {
        let v = self.v;
        let a = v.arrangement.clone();
        let (w, h) = (v.width, v.height);
        if w < 60.0 || h < 40.0 {
            return;
        }
        let (hx, ruler) = (v.header_width(), v.ruler_height());
        let step = v.bar_step();
        let bars = v.bars(step);

        // Bar lines behind everything.
        for &(bar, x) in &bars {
            let c = if bar % (step * 4) == 0 { self.p.stroke } else { self.p.hover };
            self.rect(Box2::new(x, ruler, 1.0, h - ruler), Colour::rgb(c, 1.0));
        }

        // Lanes.
        let scale = v.lane_scale();
        let mut y = ruler - v.top;
        for (i, lane) in a.lanes.iter().enumerate() {
            let lh = lane_height(lane) * scale;
            if y + lh >= ruler && y <= h {
                let keys = v.keys.get(i).copied().unwrap_or((60, 60));
                self.lane(lane, y, lh, keys);
            }
            y += lh;
        }
        if y < h {
            self.rect(Box2::new(0.0, y, w, 1.0), Colour::rgb(self.p.stroke, 1.0));
        }

        self.ruler(&a, &bars);
        self.rect(Box2::new(hx - 1.0, 0.0, 1.0, h), Colour::rgb(self.p.stroke, 1.0));

        // Thin scroll bars when zoomed in or when the lanes don't fit.
        let (tw, zoom) = (v.timeline_width(), v.zoom());
        let visible = tw / zoom;
        if visible < v.song_beats - 1e-6 {
            let bw = (tw * visible / v.song_beats).max(16.0);
            let r = Box2::new(hx + tw * v.left / v.song_beats, h - SCROLLBAR - 2.0, bw, SCROLLBAR);
            self.rounded(r, 2.5, Colour::rgb(self.p.faint, 1.0), None);
        }
        let lanes = h - ruler;
        let content = v.content_height * scale;
        if content > lanes + 1.0 {
            let bh = (lanes * lanes / content).max(16.0);
            let r = Box2::new(w - SCROLLBAR - 2.0, ruler + lanes * v.top / content, SCROLLBAR, bh);
            self.rounded(r, 2.5, Colour::rgb(self.p.faint, 1.0), None);
        }
    }

    fn ruler(&mut self, a: &Arrangement, bars: &[(u64, f64)]) {
        let v = self.v;
        let (w, h, hx, ruler) = (v.width, v.height, v.header_width(), v.ruler_height());
        self.rect(Box2::new(0.0, 0.0, w, ruler), Colour::rgb(self.p.raised, 1.0));
        self.rect(Box2::new(0.0, ruler - 1.0, w, 1.0), Colour::rgb(self.p.stroke, 1.0));
        self.label(Area::Ruler, Box2::new(12.0, 0.0, hx - 20.0, 24.0), a.view.to_uppercase(), Style::Head);
        for &(bar, x) in bars {
            self.rect(Box2::new(x, 17.0, 1.0, 6.0), Colour::rgb(self.p.muted, 1.0));
            self.label(Area::Ruler, Box2::new(x + 4.0, 0.0, 40.0, 17.0), (bar + 1).to_string(), Style::Bar);
        }
        // Markers on their own row, each name cut short where the next marker starts.
        let mut markers: Vec<_> = a.markers.iter().collect();
        markers.sort_by(|m, n| m.beat.total_cmp(&n.beat));
        for (i, m) in markers.iter().enumerate() {
            let at = v.x(m.beat);
            let next = markers.get(i + 1).map_or(w, |n| v.x(n.beat));
            if next <= hx || at > w {
                continue;
            }
            // A marker that starts left of the view still names the part the view is in: its flag is pinned to the
            // left edge, and only its line is left out.
            let x = at.max(hx);
            let room = next - x - 3.0;
            if at >= hx {
                self.rect(Box2::new(x, 24.0, 1.5, h - 24.0), Colour::rgb(self.p.warning, 1.0));
            }
            if room < 8.0 {
                continue;
            }
            let name = if m.name.is_empty() { "Marker" } else { m.name.as_str() };
            // About how wide the name is at the ruler's size; the window cuts it to the room there is.
            let wide = (name.chars().count() as f64 * 6.2 + 8.0).min(room);
            self.rounded(Box2::new(x, 24.0, wide, 15.0), 3.0, Colour::rgb(self.p.warning, 1.0), None);
            self.label(Area::Ruler, Box2::new(x + 4.0, 24.0, wide - 6.0, 15.0), name, Style::Marker);
        }
    }

    /// The colour a clip is drawn in: its own, else its lane's, else one for what the lane holds.
    fn colour_of(&self, lane: &PreviewLane, clip: Option<&PreviewClip>) -> u32 {
        clip.and_then(|c| c.colour).or(lane.colour).unwrap_or(match lane.kind {
            LaneKind::Audio => self.p.ok,
            LaneKind::Automation => self.p.warning,
            _ => self.p.accent,
        })
    }

    /// Draws one lane: its name on the left, its clips on the timeline.
    fn lane(&mut self, lane: &PreviewLane, y: f64, lh: f64, keys: (u8, u8)) {
        let v = self.v;
        let (w, hx, ruler) = (v.width, v.header_width(), v.ruler_height());
        let c = self.colour_of(lane, None);
        if lane.kind == LaneKind::Group {
            self.rect(Box2::new(0.0, y, w, lh - 1.0), Colour::rgb(c, 0.08));
        }
        self.rect(Box2::new(0.0, y + lh - 1.0, w, 1.0), Colour::rgb(self.p.stroke, 1.0));

        // The name, with a strip of the lane's colour like a DAW's track header.
        let indent = 10.0 + lane.depth.min(8) as f64 * 12.0;
        let inset = (lh * 0.15).min(6.0);
        self.rounded(
            Box2::new(indent - 6.0, y + inset, 3.0, (lh - 2.0 * inset - 1.0).max(1.0)),
            1.5,
            Colour::rgb(c, 1.0),
            None,
        );
        let style = Style::Lane {
            group: lane.kind == LaneKind::Group,
            muted: lane.muted,
            small: lane.kind == LaneKind::Automation,
        };
        let room = hx - indent - 10.0;
        if !lane.detail.is_empty() && lh >= 40.0 {
            self.label(Area::Header, Box2::new(indent, y + lh / 2.0 - 16.0, room, 16.0), lane.name.clone(), style);
            self.label(Area::Header, Box2::new(indent, y + lh / 2.0, room, 15.0), lane.detail.clone(), Style::Detail);
        } else if lh >= 14.0 {
            // Squeezed lanes are too thin for names.
            self.label(Area::Header, Box2::new(indent, y, room, lh), lane.name.clone(), style);
        }

        let area = Box2::new(hx, y.max(ruler), w - hx, (y + lh).min(v.height) - y.max(ruler));
        for clip in &lane.clips {
            let (x0, x1) = (v.x(clip.start), v.x(clip.end()));
            if x1 < hx || x0 > w {
                continue;
            }
            let r = v.clip_box(clip, y, lh);
            let c = self.colour_of(lane, Some(clip));
            self.clip(clip, r, area, c, keys);
        }
    }

    fn clip(&mut self, clip: &PreviewClip, r: Box2, area: Box2, c: u32, keys: (u8, u8)) {
        let fade = if clip.muted { 0.35 } else { 1.0 };
        // Only what shows of the clip is drawn; one cut by the edges is drawn square there.
        let Some(shown) = r.intersect(&area) else { return };
        self.rounded(shown, 3.0, Colour::rgb(c, 0.24 * fade), Some(Colour::rgb(c, 0.75 * fade)));
        let title = if r.h >= 30.0 && r.w > 28.0 { 13.0 } else { 0.0 };
        if title > 0.0 {
            if let Some(bar) = Box2::new(r.x, r.y, r.w, title).intersect(&area) {
                self.rect(bar, Colour::rgb(c, 0.55 * fade));
            }
            if !clip.name.is_empty() && r.w > 34.0 && self.clip_labels < MOST_CLIP_LABELS {
                let x = r.x.max(area.x) + 4.0;
                let room = r.right().min(area.right()) - x - 4.0;
                if room > 12.0 {
                    self.clip_labels += 1;
                    self.label(Area::Timeline, Box2::new(x, r.y, room, title), clip.name.clone(), Style::Clip);
                }
            }
        }
        let inner = Box2::new(r.x + 2.0, r.y + title + 2.0, (r.w - 4.0).max(1.0), (r.h - title - 4.0).max(1.0));
        let Some(visible) = inner.intersect(&area) else { return };
        let fill = Colour::rgb(if self.p.light { darken(c, 0.35) } else { lighten(c, 0.25) }, fade);
        if !clip.notes.is_empty() {
            self.notes(clip, inner, visible, fill, keys);
        }
        for part in &clip.audio {
            self.audio(part, clip, inner, visible, c, fade);
        }
        if clip.points.len() > 1 {
            self.curve(clip, inner, visible, fill);
        }
        if clip.muted && r.w > 40.0 && r.right() <= area.right() {
            self.label(
                Area::Timeline,
                Box2::new(r.right() - 36.0, r.bottom() - 13.0, 34.0, 12.0),
                "muted",
                Style::Small,
            );
        }
    }

    /// Draws a clip's notes as a little piano roll, spread over the lane's lowest to highest key.
    fn notes(&mut self, clip: &PreviewClip, inner: Box2, visible: Box2, fill: Colour, keys: (u8, u8)) {
        let v = self.v;
        let (lo, hi) = (keys.0 as f64, keys.1 as f64);
        let rows = (hi - lo + 1.0).max(1.0);
        let row_h = (inner.h / rows).clamp(1.2, 5.0);
        let span = inner.h - row_h;
        let zoom = v.zoom();
        let mut runs = Runs::new(self.scale);
        for n in clip.notes.iter().filter(|n| !n.muted) {
            let x = v.x(clip.start + n.start);
            let nw = (n.length * zoom - 0.5).max(1.2);
            let ny = inner.y + if rows <= 1.0 { span / 2.0 } else { span * (1.0 - (n.key as f64 - lo) / (rows - 1.0)) };
            if let Some(b) = Box2::new(x, ny, nw, row_h).intersect(&visible) {
                runs.add(b);
            }
        }
        for r in runs.boxes() {
            self.rect(r, fill);
        }
    }

    fn audio(&mut self, part: &AudioPart, clip: &PreviewClip, inner: Box2, visible: Box2, c: u32, fade: f64) {
        let v = self.v;
        let x0 = v.x(clip.start + part.start).max(visible.x);
        let x1 = v.x(clip.start + part.start + part.length).min(visible.right());
        if x1 <= x0 {
            return;
        }
        let mid = inner.y + inner.h / 2.0;
        if let Some(peaks) = v.peaks.get(&part.path)
            && self.waveform(part, clip, peaks, (x0, x1), inner, visible, c, fade)
        {
            return;
        }
        if v.found.contains_key(&part.path) {
            // The sound itself is drawn once its file has been read; until then (or when it can't be) a soft band.
            let band = Box2::new(x0, mid - inner.h * 0.3, x1 - x0, inner.h * 0.6);
            if let Some(b) = band.intersect(&visible) {
                self.rect(b, Colour::rgb(c, 0.22 * fade));
            }
            return;
        }
        // No file: a flat line and a note, so it's clear the clip is there but its sound isn't.
        if let Some(b) = Box2::new(x0, mid, x1 - x0, 1.0).intersect(&visible) {
            self.rect(b, Colour::rgb(c, 0.6 * fade));
        }
        if x1 - x0 > 120.0 && visible.h >= 12.0 {
            // Just above the line, or at the top of a clip too thin for that.
            let y = (mid - 13.0).max(visible.y).min(visible.bottom() - 12.0);
            let r = Box2::new(x0 + 4.0, y, x1 - x0 - 8.0, 12.0);
            self.label(Area::Timeline, r, "file not on this computer", Style::Small);
        }
    }

    /// Draws an audio part's waveform, a thin bar a pixel, mirrored about the clip's middle. False when the file's
    /// speed in the song can't be told, so the caller draws the plain band.
    #[allow(clippy::too_many_arguments)]
    fn waveform(
        &mut self,
        part: &AudioPart,
        clip: &PreviewClip,
        peaks: &Peaks,
        (x0, x1): (f64, f64),
        inner: Box2,
        visible: Box2,
        c: u32,
        fade: f64,
    ) -> bool {
        let rate = part.rate(peaks.file_seconds);
        if rate <= 0.0 {
            return false;
        }
        let v = self.v;
        let mid = inner.y + inner.h / 2.0;
        let half = inner.h * 0.46;
        // Seconds into the file at beat `b` of the song (a reversed part plays from its end back).
        let start = clip.start + part.start;
        let at = |b: f64| {
            let into = (b - start).clamp(0.0, part.length);
            part.from + (if part.reversed { part.length - into } else { into }) * rate
        };
        let fill = Colour::rgb(if self.p.light { darken(c, 0.35) } else { lighten(c, 0.25) }, 0.9 * fade);
        // A column for each pixel, on the pixel grid.
        let column = 1.0 / self.scale;
        let (first, last) = ((x0 * self.scale).floor() as i64, (x1 * self.scale).ceil() as i64);
        for i in first..last {
            let x = i as f64 * column;
            let (b0, b1) = (v.beat(x), v.beat(x + column));
            if let Some(loud) = peaks.loudest(at(b0), at(b1)) {
                let h = (f64::from(loud) * half).max(0.5);
                if let Some(b) = Box2::new(x, mid - h, column, 2.0 * h).intersect(&visible) {
                    self.rect(b, fill);
                }
            }
        }
        true
    }

    fn curve(&mut self, clip: &PreviewClip, inner: Box2, visible: Box2, fill: Colour) {
        let v = self.v;
        let points: Vec<(f64, f64)> = clip
            .points
            .iter()
            .map(|p| (v.x(clip.start + p.beat), inner.bottom() - p.value.clamp(0.0, 1.0) * inner.h))
            .collect();
        for pair in points.windows(2) {
            let [a, b] = pair else { continue };
            if let Some((from, to)) = clip_segment(*a, *b, &visible) {
                self.scene.line(from, to, 1.5, fill);
            }
        }
    }
}

/// The part of the line from `a` to `b` inside `r` (Liang and Barsky's way), or none when it's all outside.
fn clip_segment(a: (f64, f64), b: (f64, f64), r: &Box2) -> Option<((f64, f64), (f64, f64))> {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    for (p, q) in [(-dx, a.0 - r.x), (dx, r.right() - a.0), (-dy, a.1 - r.y), (dy, r.bottom() - a.1)] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
        } else {
            let t = q / p;
            if p < 0.0 {
                t0 = t0.max(t);
            } else {
                t1 = t1.min(t);
            }
        }
    }
    (t0 <= t1).then_some(((a.0 + t0 * dx, a.1 + t0 * dy), (a.0 + t1 * dx, a.1 + t1 * dy)))
}

/// One line about a clip for the hover label: "Verse · 12 notes", "Vox.wav (not on this computer)", "muted".
fn describe(lane: &PreviewLane, clip: &PreviewClip, found: &HashMap<String, PathBuf>) -> String {
    let mut bits = vec![if clip.name.is_empty() { lane.name.clone() } else { clip.name.clone() }];
    if !clip.notes.is_empty() {
        bits.push(count(clip.notes.iter().filter(|n| !n.muted).count() as u64, "note"));
    }
    if let Some(part) = clip.audio.first() {
        let name = mo_formats::paths::file_name(&part.path).to_owned();
        bits.push(if found.contains_key(&part.path) { name } else { format!("{name} (not on this computer)") });
    }
    if !clip.points.is_empty() {
        bits.push("automation".to_owned());
    }
    if clip.muted {
        bits.push("muted".to_owned());
    }
    bits.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_waveform_keeps_the_loudest_of_each_slice() {
        // 1,000 frames a second: a slice is 5 frames. Quiet, then a loud click in the second slice of the right channel.
        let left = vec![0.1f32; 20];
        let mut right = vec![-0.05f32; 20];
        right[7] = -0.9;
        let p = Peaks::of(1000, &[left, right], 0.02);
        assert_eq!(p.values.len(), 4);
        assert!((p.step - 0.005).abs() < 1e-9);
        assert_eq!(p.values[1], 0.9);
        assert_eq!(p.values[0], 0.1);
        assert_eq!(p.loudest(0.0, 0.004), Some(0.1));
        assert_eq!(p.loudest(0.004, 0.0), Some(0.1));
        assert_eq!(p.loudest(0.0, 0.02), Some(0.9));
        // Past what was read.
        assert_eq!(p.loudest(0.03, 0.04), None);
    }
    use mo_formats::arrangement::{PreviewMarker, PreviewNote, PreviewPoint};

    fn note(start: f64, key: u8) -> PreviewNote {
        PreviewNote { start, length: 1.0, key, velocity: 40000, muted: false }
    }

    fn song() -> Arrangement {
        let mut keys = PreviewLane::new("Keys", LaneKind::Instrument);
        keys.detail = "Serum".into();
        keys.colour = Some(0x3366CC);
        keys.clips = vec![
            PreviewClip {
                name: "Verse".into(),
                start: 0.0,
                length: 16.0,
                notes: vec![note(0.0, 60), note(4.0, 72)],
                ..PreviewClip::default()
            },
            PreviewClip {
                name: "Chorus".into(),
                start: 16.0,
                length: 16.0,
                muted: true,
                notes: vec![note(0.0, 64)],
                ..PreviewClip::default()
            },
        ];
        let mut vox = PreviewLane::new("Vox", LaneKind::Audio);
        vox.clips = vec![PreviewClip {
            name: "Vox".into(),
            start: 8.0,
            length: 8.0,
            audio: vec![AudioPart {
                path: r"C:\Samples\Vox.wav".into(),
                start: 0.0,
                length: 8.0,
                from: 0.0,
                seconds_per_beat: Some(0.5),
                whole_file_beats: None,
                reversed: false,
            }],
            ..PreviewClip::default()
        }];
        let mut cutoff = PreviewLane::new("Cutoff", LaneKind::Automation);
        cutoff.clips = vec![PreviewClip {
            name: "Cutoff".into(),
            start: 0.0,
            length: 32.0,
            points: vec![PreviewPoint { beat: 0.0, value: 0.0 }, PreviewPoint { beat: 32.0, value: 1.0 }],
            ..PreviewClip::default()
        }];
        Arrangement {
            view: "FL Studio playlist",
            tempo: 140.0,
            beats_per_bar: 4,
            lanes: vec![keys, vox, cutoff],
            markers: vec![PreviewMarker { beat: 16.0, name: "Drop".into() }],
            notes: Vec::new(),
        }
    }

    fn view(width: f64, height: f64) -> View {
        let mut v = View::new(Arc::new(song()), Arc::default());
        v.resize(width, height);
        v
    }

    fn pixel(p: &tiny_skia::Pixmap, x: u32, y: u32) -> [u8; 4] {
        let c = p.pixel(x, y).expect("inside the picture");
        [c.red(), c.green(), c.blue(), c.alpha()]
    }

    fn palette() -> Palette {
        Palette::from_look(&Look::from_settings(None, None))
    }

    /// Notes in the same pixels are drawn as one rectangle: as few as the pixels they fill, and those very pixels,
    /// see-through ones (a muted clip's) included.
    #[test]
    fn notes_in_the_same_pixels_are_drawn_as_one() {
        let scale = 1.5;
        let fill = Colour::rgb(0xFFFFFF, 0.35);
        let (mut each, mut merged) = (Scene::new(100.0, 12.0, 0x101010), Scene::new(100.0, 12.0, 0x101010));
        let mut runs = Runs::new(scale);
        // 2,000 notes a sixteenth of a point apart in two rows, then two more in the first row, apart.
        let notes = (0..2000)
            .map(|i| Box2::new(f64::from(i % 1000) * 0.0625, if i < 1000 { 2.0 } else { 6.0 }, 1.25, 1.25))
            .chain([Box2::new(80.0, 2.0, 1.25, 1.25), Box2::new(90.0, 2.0, 1.25, 1.25)]);
        for b in notes {
            each.rect((b.x, b.y, b.w, b.h), fill);
            runs.add(b);
        }
        for b in runs.boxes() {
            merged.rect((b.x, b.y, b.w, b.h), fill);
        }
        assert_eq!(merged.shapes.len(), 4, "a run in each row, and the two apart");
        assert!(merged.paint(scale).data() == each.paint(scale).data(), "the same pixels");
    }

    /// A clip far too dense for the view, fitted, is drawn as about one rectangle a run of pixels.
    #[test]
    fn a_huge_clip_is_as_many_rectangles_as_the_view_has_room_for() {
        let mut song = song();
        song.lanes[0].clips[0].notes = (0..100_000).map(|i| note(f64::from(i) * 0.00016, 60 + (i % 4) as u8)).collect();
        let mut v = View::new(Arc::new(song), Arc::default());
        v.resize(900.0, 300.0);
        let shapes = v.frame(&palette(), 2.0).scene.shapes.len();
        assert!(shapes < 500, "{shapes} shapes for 100,000 notes");
    }

    #[test]
    fn a_clip_ending_at_1e20_beats_neither_panics_nor_hangs() {
        let mut song = song();
        song.lanes[0].clips[1].start = 1e20;
        let mut v = View::new(Arc::new(song), Arc::default());
        v.resize(900.0, 300.0);
        v.moved(880.0, 10.0);
        assert!(v.hover().is_some_and(|h| h.text.starts_with("Bar ")));
        v.frame(&palette(), 1.0);
        // Zoomed in and scrolled far to the right, where the bars are drawn.
        for _ in 0..40 {
            v.wheel(800.0, 0.0, 5.0, true, false);
        }
        v.wheel(800.0, -1e12, 0.0, false, true);
        v.moved(880.0, 10.0);
        let _ = v.hover();
        v.frame(&palette(), 1.0);
    }

    #[test]
    fn sums_up_the_song() {
        assert_eq!(summary(&song()), "140 BPM · 8 bars · 0:14 · 3 tracks · 4 clips · 2 notes");
        let mut a = song();
        a.tempo = 92.5;
        a.lanes.clear();
        assert_eq!(summary(&a), "92.5 BPM · 0 tracks · 0 clips");
        assert_eq!(count(1204, "note"), "1,204 notes");
    }

    #[test]
    fn the_song_fits_the_width_and_zooms_about_the_mouse() {
        let mut v = view(1000.0, 300.0);
        // 32 beats and a little room after: the last clip's end is inside the view.
        assert!(v.x(32.0) < 1000.0 - 8.0);
        assert_eq!(v.x(0.0), 172.0);
        let under = v.beat(500.0);
        v.wheel(500.0, 0.0, 2.0, true, false);
        assert!((v.beat(500.0) - under).abs() < 1e-9, "the beat under the mouse stays put");
        assert!(v.zoom() > v.fit_zoom());
        // Zooming out never goes past the whole song.
        v.wheel(500.0, 0.0, -20.0, true, false);
        assert_eq!(v.zoom(), v.fit_zoom());
        assert_eq!(v.left, 0.0);
        // ...and stays fitted when the view is made wider.
        v.resize(1400.0, 300.0);
        assert_eq!(v.zoom(), v.fit_zoom());
        // A narrow view has a narrower header.
        assert_eq!(view(400.0, 300.0).header_width(), 118.0);
    }

    #[test]
    fn dragging_moves_around_and_a_double_click_fits_everything() {
        let mut v = view(800.0, 120.0);
        v.wheel(400.0, 0.0, 3.0, true, false);
        v.press(400.0, 80.0);
        v.moved(300.0, 40.0);
        assert!(v.left > 0.0);
        assert!(v.top > 0.0, "three lanes don't fit 120 points");
        v.release();
        v.double_click();
        assert_eq!((v.left, v.top, v.zoom), (0.0, 0.0, 0.0));
        assert!(v.lane_scale() < 1.0, "every lane squeezed in");
        v.double_click();
        assert_eq!(v.lane_scale(), 1.0, "again goes back");
    }

    #[test]
    fn draws_clips_in_their_colours_with_names_laid_on_top() {
        let mut v = view(800.0, 200.0);
        v.moved(300.0, 50.0);
        let f = v.frame(&palette(), 2.0);
        let picture = f.scene.paint(2.0);
        assert_eq!((picture.width(), picture.height()), (1600, 400));
        let names: Vec<_> = f.labels.iter().map(|l| (l.area, l.text.as_str())).collect();
        for expected in [
            (Area::Header, "Keys"),
            (Area::Header, "Serum"),
            (Area::Header, "Vox"),
            (Area::Timeline, "Verse"),
            (Area::Timeline, "muted"),
            (Area::Timeline, "file not on this computer"),
            (Area::Ruler, "FL STUDIO PLAYLIST"),
            (Area::Ruler, "1"),
            (Area::Ruler, "Drop"),
        ] {
            assert!(names.contains(&expected), "{expected:?} in {names:?}");
        }
        // The Verse clip's title bar is the lane's blue, over the surface.
        let x = v.x(2.0);
        let px = pixel(&picture, (x * 2.0) as u32, ((v.ruler_height() + 6.0) * 2.0) as u32);
        assert!(px[2] > px[0] && px[2] > px[1], "{px:?}");
        assert_eq!(f.hover, v.hover(), "the window asks the view itself between pictures");
        let hover = f.hover.expect("the mouse is over a clip");
        assert!(hover.text.starts_with("Bar "), "{}", hover.text);
        assert!(hover.text.ends_with("Verse · 2 notes"), "{}", hover.text);
    }

    #[test]
    fn a_marker_left_of_the_view_still_names_the_part_it_is_in() {
        let mut v = view(800.0, 200.0);
        v.wheel(700.0, 0.0, 8.0, true, false);
        assert!(v.left > 16.0, "scrolled past the Drop marker at beat 16");
        let f = v.frame(&palette(), 1.0);
        let drop = f.labels.iter().find(|l| l.text == "Drop").expect("the Drop flag is pinned at the left");
        assert!(drop.x >= v.header_width());
    }

    #[test]
    fn a_tiny_view_draws_nothing_but_its_background() {
        let f = view(30.0, 20.0).frame(&palette(), 1.0);
        assert!(f.labels.is_empty());
        assert!(f.scene.shapes.is_empty());
        assert_eq!(f.scene.pixel_size(1.0), (30, 20));
    }

    #[test]
    fn lines_are_cut_to_a_box() {
        let r = Box2::new(0.0, 0.0, 10.0, 10.0);
        assert_eq!(clip_segment((-5.0, 5.0), (15.0, 5.0), &r), Some(((0.0, 5.0), (10.0, 5.0))));
        assert_eq!(clip_segment((-5.0, -5.0), (-1.0, 20.0), &r), None);
        assert_eq!(clip_segment((2.0, 2.0), (3.0, 3.0), &r), Some(((2.0, 2.0), (3.0, 3.0))));
    }
}
