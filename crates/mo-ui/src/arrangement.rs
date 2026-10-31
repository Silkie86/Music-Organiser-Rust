//! Fills the arrangement on a song's Overview. Picking a song asks a background thread to read its newest save
//! (newest ask wins, rule 12); the mouse then moves the view here on the window's thread, and each change asks a
//! second background thread to draw it. The picture and its words come back and are put on the page, unless another
//! song has been picked meanwhile.

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::Arc;

use mo_app::arrangement::{self as arr, Area, Frame, Hover, Palette, Preview, Style, View};
use mo_app::background::Newest;
use mo_app::log;
use mo_app::themes::Look;
use slint::{ComponentHandle, Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel};

use crate::{AppWindow, ArrKind, ArrLabel, ArrangementData};

/// What the page says when there's no save to read.
const NOT_HERE: &str = "This save isn't on this computer, so its arrangement can't be drawn.";
const READING: &str = "Reading the arrangement…";
const NOT_YET: &str = "Arrangements can be drawn for FL Studio, Bitwig and Studio One projects.";

/// A picture to draw: for which song, the view as it is, its colours and pixels to a point.
type DrawJob = (u64, View, Palette, f64);

#[derive(Default)]
struct State {
    /// Counts the songs shown, so a read or a picture that arrives after another song was picked is dropped.
    asked: u64,
    preview: Option<Preview>,
    view: Option<View>,
    /// The size the page gives the picture, in points.
    size: (f64, f64),
    palette: Option<Palette>,
    /// Read and draw here instead of on the background threads: for the screenshots, which have no event loop.
    here: bool,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::default();
    static READER: RefCell<Option<Newest<(u64, PathBuf)>>> = const { RefCell::new(None) };
    static DRAWER: RefCell<Option<Newest<DrawJob>>> = const { RefCell::new(None) };
}

/// Wires the preview's mouse and view chips, and starts its reading and drawing threads.
pub fn connect(app: &AppWindow) {
    let data = app.global::<ArrangementData>();
    let weak = app.as_weak();
    data.on_pick_view(move |i| {
        if let Some(app) = weak.upgrade() {
            pick_view(&app, usize::try_from(i).unwrap_or(0));
        }
    });
    let weak = app.as_weak();
    data.on_resized(move |w, h| {
        change(&weak, |s| s.size = (f64::from(w), f64::from(h)));
    });
    let weak = app.as_weak();
    data.on_wheel(move |x, across, down, ctrl, shift| {
        with_view(&weak, |v| {
            v.wheel(f64::from(x), f64::from(across), f64::from(down), ctrl, shift);
            true
        });
    });
    let weak = app.as_weak();
    data.on_grab(move |x, y| {
        with_view(&weak, |v| {
            v.press(f64::from(x), f64::from(y));
            false
        });
    });
    // Only a drag changes the picture; the mouse just moving changes what's under it, which the window shows itself.
    let weak = app.as_weak();
    data.on_moved(move |x, y| {
        with_view(&weak, |v| {
            v.moved(f64::from(x), f64::from(y));
            v.dragging()
        });
    });
    let weak = app.as_weak();
    data.on_let_go(move || {
        with_view(&weak, |v| {
            v.release();
            false
        });
    });
    let weak = app.as_weak();
    data.on_double_clicked(move || {
        with_view(&weak, |v| {
            v.double_click();
            true
        });
    });
    let weak = app.as_weak();
    data.on_left(move || {
        with_view(&weak, |v| {
            v.leave();
            false
        });
    });

    let weak = app.as_weak();
    let reader = Newest::start("Arrangement", move |(asked, path): (u64, PathBuf)| {
        let read = arr::read(&path, Some(&mo_app::paths::data_dir().join("sounds.db")));
        let _ = weak.upgrade_in_event_loop(move |app| arrived(&app, asked, read));
    });
    match reader {
        Ok(r) => READER.with(|slot| *slot.borrow_mut() = Some(r)),
        Err(e) => log::error("Arrangement", &format!("Couldn't start the arrangement's reader: {e}")),
    }
    let weak = app.as_weak();
    let drawer = Newest::start("Arrangement drawing", move |(asked, view, palette, scale): DrawJob| {
        // The picture is copied into the window's kind of buffer here, so the window's thread only puts it up.
        let picture = Picture::from(view.frame(&palette, scale));
        let _ = weak.upgrade_in_event_loop(move |app| drawn(&app, asked, picture));
    });
    match drawer {
        Ok(d) => DRAWER.with(|slot| *slot.borrow_mut() = Some(d)),
        Err(e) => log::error("Arrangement", &format!("Couldn't start drawing the arrangement: {e}")),
    }
}

/// Uses a look's colours, and draws again in them.
pub fn set_look(app: &AppWindow, look: &Look) {
    STATE.with(|s| s.borrow_mut().palette = Some(Palette::from_look(look)));
    redraw(app);
}

/// Reads and draws on the window's thread from now on: for the screenshots, which have no event loop to post to.
pub fn draw_here() {
    STATE.with(|s| s.borrow_mut().here = true);
}

/// Shows the arrangement of the save at `path` (the picked song's newest), or empties the preview when no song is
/// picked or its save has gone.
pub fn show_song(app: &AppWindow, path: Option<&str>, missing: bool) {
    let data = app.global::<ArrangementData>();
    let (asked, here) = STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.asked += 1;
        s.preview = None;
        s.view = None;
        (s.asked, s.here)
    });
    clear(&data);
    let Some(path) = path.filter(|p| !p.is_empty()) else { return };
    if missing {
        data.set_message(NOT_HERE.into());
        return;
    }
    data.set_message(READING.into());
    let path = PathBuf::from(path);
    if here {
        // The screenshots: their made-up songs, without the real Sounds library.
        arrived(app, asked, arr::read(&path, None));
        return;
    }
    READER.with(|slot| {
        if let Some(reader) = slot.borrow().as_ref() {
            reader.ask((asked, path));
        }
    });
}

/// Shows an arrangement already read: for the screenshots.
pub fn show_preview(app: &AppWindow, preview: Preview) {
    let asked = STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.asked += 1;
        s.asked
    });
    arrived(app, asked, Ok(preview));
}

fn clear(data: &ArrangementData) {
    data.set_views(ModelRc::default());
    data.set_view(0);
    data.set_summary(SharedString::new());
    data.set_notes(ModelRc::default());
    data.set_picture(Image::default());
    data.set_header_labels(ModelRc::default());
    data.set_timeline_labels(ModelRc::default());
    data.set_ruler_labels(ModelRc::default());
    data.set_hover_text(SharedString::new());
    data.set_dragging(false);
    data.set_message(SharedString::new());
}

/// A save's arrangement has been read: shown if its song is still the one picked.
fn arrived(app: &AppWindow, asked: u64, read: Result<Preview, String>) {
    if STATE.with(|s| s.borrow().asked != asked) {
        return;
    }
    let data = app.global::<ArrangementData>();
    let preview = match read {
        Ok(p) => p,
        Err(e) => {
            log::warning("Arrangement", &e);
            data.set_message(e.into());
            return;
        }
    };
    if preview.views.is_empty() {
        data.set_message(NOT_YET.into());
        return;
    }
    data.set_views(strings(preview.views.iter().map(|a| a.view.to_owned())));
    STATE.with(|s| s.borrow_mut().preview = Some(preview));
    pick_view(app, 0);
}

/// Shows the project's view at `index` (the FL playlist, or as it opens in Bitwig), fitted to the width.
fn pick_view(app: &AppWindow, index: usize) {
    let data = app.global::<ArrangementData>();
    let shown = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let p = s.preview.as_ref()?;
        let a = p.views.get(index)?.clone();
        let mut view = View::new(a.clone(), p.found.clone());
        view.resize(s.size.0, s.size.1);
        s.view = Some(view);
        Some(a)
    });
    let Some(a) = shown else { return };
    data.set_view(i32::try_from(index).unwrap_or(0));
    data.set_summary(arr::summary(&a).into());
    if a.lanes.is_empty() {
        // Nothing to draw: the reason, when there is one, takes the picture's place.
        let why = a.notes.join(" ");
        data.set_notes(ModelRc::default());
        data.set_message(if why.is_empty() {
            "Nothing has been placed in this arrangement yet.".into()
        } else {
            why.into()
        });
        return;
    }
    data.set_notes(strings(a.notes.iter().cloned()));
    data.set_message(SharedString::new());
    redraw(app);
}

/// Changes what the view knows and draws it again.
fn change(weak: &slint::Weak<AppWindow>, f: impl FnOnce(&mut State)) {
    let Some(app) = weak.upgrade() else { return };
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        f(&mut s);
        let size = s.size;
        if let Some(v) = s.view.as_mut() {
            v.resize(size.0, size.1);
        }
    });
    redraw(&app);
}

/// Tells the view what the mouse did, shows what's under it now, and draws it again when `f` says the picture changed.
fn with_view(weak: &slint::Weak<AppWindow>, f: impl FnOnce(&mut View) -> bool) {
    let Some(app) = weak.upgrade() else { return };
    let done = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let v = s.view.as_mut()?;
        let changed = f(v);
        Some((changed, v.dragging(), v.hover()))
    });
    let Some((changed, dragging, hover)) = done else { return };
    let data = app.global::<ArrangementData>();
    data.set_dragging(dragging);
    show_hover(&data, hover);
    if changed {
        redraw(&app);
    }
}

fn show_hover(data: &ArrangementData, hover: Option<Hover>) {
    match hover {
        Some(h) => {
            data.set_hover_x(h.x as f32);
            data.set_hover_y(h.y as f32);
            data.set_hover_text(h.text.into());
        }
        None => data.set_hover_text(SharedString::new()),
    }
}

/// Asks for a picture of the view as it is now.
fn redraw(app: &AppWindow) {
    let scale = f64::from(app.window().scale_factor());
    let job = STATE.with(|s| {
        let s = s.borrow();
        let view = s.view.clone()?;
        (view.width >= 1.0 && view.height >= 1.0).then_some(())?;
        Some((s.asked, view, s.palette?, scale, s.here))
    });
    let Some((asked, view, palette, scale, here)) = job else { return };
    if here {
        drawn(app, asked, Picture::from(view.frame(&palette, scale)));
        return;
    }
    DRAWER.with(|slot| {
        if let Some(drawer) = slot.borrow().as_ref() {
            drawer.ask((asked, view, palette, scale));
        }
    });
}

/// A drawn view, ready for the window: made on the drawing thread.
struct Picture {
    pixels: Option<SharedPixelBuffer<Rgba8Pixel>>,
    header: Vec<ArrLabel>,
    timeline: Vec<ArrLabel>,
    ruler: Vec<ArrLabel>,
    header_width: f32,
    ruler_height: f32,
}

impl From<Frame> for Picture {
    fn from(frame: Frame) -> Picture {
        let Frame { pixel_width, pixel_height, rgba, labels, header_width, ruler_height, .. } = frame;
        let pixels = (rgba.len() == (pixel_width as usize) * (pixel_height as usize) * 4)
            .then(|| SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&rgba, pixel_width, pixel_height));
        let (mut header, mut timeline, mut ruler) = (Vec::new(), Vec::new(), Vec::new());
        for l in labels {
            let (kind, muted) = kind(l.style);
            let line = ArrLabel {
                x: l.x as f32,
                y: l.y as f32,
                width: l.width as f32,
                height: l.height as f32,
                text: l.text.into(),
                kind,
                muted,
            };
            match l.area {
                Area::Header => header.push(line),
                Area::Timeline => timeline.push(line),
                Area::Ruler => ruler.push(line),
            }
        }
        Picture {
            pixels,
            header,
            timeline,
            ruler,
            header_width: header_width as f32,
            ruler_height: ruler_height as f32,
        }
    }
}

/// A picture has been drawn: put on the page with its words, unless another song has been picked since.
fn drawn(app: &AppWindow, asked: u64, picture: Picture) {
    if STATE.with(|s| s.borrow().asked != asked) {
        return;
    }
    let data = app.global::<ArrangementData>();
    if let Some(pixels) = picture.pixels {
        data.set_picture(Image::from_rgba8_premultiplied(pixels));
    }
    data.set_header_width(picture.header_width);
    data.set_ruler_height(picture.ruler_height);
    data.set_header_labels(ModelRc::new(VecModel::from(picture.header)));
    data.set_timeline_labels(ModelRc::new(VecModel::from(picture.timeline)));
    data.set_ruler_labels(ModelRc::new(VecModel::from(picture.ruler)));
    // What's under the mouse now, which may have moved on since this picture was asked for.
    show_hover(&data, STATE.with(|s| s.borrow().view.as_ref().and_then(View::hover)));
}

/// A label's look, and whether it's greyed out.
fn kind(style: Style) -> (ArrKind, bool) {
    match style {
        Style::Lane { group: true, muted, .. } => (ArrKind::Group, muted),
        Style::Lane { small: true, muted, .. } => (ArrKind::Automation, muted),
        Style::Lane { muted, .. } => (ArrKind::Lane, muted),
        Style::Detail => (ArrKind::Detail, false),
        Style::Head => (ArrKind::Head, false),
        Style::Bar => (ArrKind::Bar, false),
        Style::Marker => (ArrKind::Marker, false),
        Style::Clip => (ArrKind::Clip, false),
        Style::Small => (ArrKind::Small, false),
    }
}

fn strings(items: impl Iterator<Item = String>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(items.map(SharedString::from).collect::<Vec<_>>()))
}

/// A made-up arrangement for the screenshots: names and notes invented here (rule 4).
pub fn sample_preview() -> Preview {
    use mo_formats::arrangement::{
        Arrangement, AudioPart, LaneKind, PreviewClip, PreviewLane, PreviewMarker, PreviewNote, PreviewPoint,
    };
    let note = |start: f64, key: u8, length: f64| PreviewNote { start, length, key, velocity: 48000, muted: false };
    let pattern = |name: &str, start: f64, length: f64, notes: Vec<PreviewNote>| PreviewClip {
        name: name.into(),
        start,
        length,
        notes,
        ..PreviewClip::default()
    };
    let beat = |bar: f64| bar * 4.0;
    let mut drums = PreviewLane::new("Drums", LaneKind::Group);
    drums.colour = Some(0xE0645A);
    let mut kick = PreviewLane::new("Kick", LaneKind::Instrument);
    kick.depth = 1;
    kick.detail = "FPC".into();
    kick.colour = Some(0xE0645A);
    let kicks: Vec<_> = (0..16).map(|i| note(i as f64, 36, 0.5)).collect();
    kick.clips = (0..6).map(|i| pattern("Kick", beat(8.0 + i as f64 * 4.0), 16.0, kicks.clone())).collect();
    let mut hats = PreviewLane::new("Hats", LaneKind::Instrument);
    hats.depth = 1;
    hats.detail = "FPC".into();
    hats.colour = Some(0xE89A5A);
    let hat: Vec<_> = (0..32).map(|i| note(i as f64 * 0.5, 42 + (i % 2) as u8 * 4, 0.25)).collect();
    hats.clips = (0..5).map(|i| pattern("Hats", beat(12.0 + i as f64 * 4.0), 16.0, hat.clone())).collect();
    let mut bass = PreviewLane::new("Bass", LaneKind::Instrument);
    bass.detail = "Serum".into();
    bass.colour = Some(0x5EA8FF);
    let line: Vec<_> =
        [36u8, 36, 43, 41, 36, 39, 43, 34].iter().enumerate().map(|(i, k)| note(i as f64 * 2.0, *k, 1.5)).collect();
    bass.clips = (0..4).map(|i| pattern("Bass line", beat(16.0 + i as f64 * 4.0), 16.0, line.clone())).collect();
    let mut keys = PreviewLane::new("Keys", LaneKind::Instrument);
    keys.detail = "Keyscape".into();
    keys.colour = Some(0x9B7BFF);
    let chords: Vec<_> = [[60u8, 64, 67], [57, 60, 64], [62, 65, 69], [59, 62, 67]]
        .iter()
        .enumerate()
        .flat_map(|(i, c)| c.iter().map(move |k| note(i as f64 * 4.0, *k, 3.5)))
        .collect();
    keys.clips = vec![
        pattern(
            "Intro chords",
            0.0,
            beat(8.0),
            [chords.clone(), chords.iter().map(|n| PreviewNote { start: n.start + 16.0, ..*n }).collect()].concat(),
        ),
        pattern("Chords", beat(16.0), beat(8.0), chords.clone()),
        PreviewClip { muted: true, ..pattern("Chords (old)", beat(24.0), beat(4.0), chords.clone()) },
    ];
    let mut vox = PreviewLane::new("Vocal", LaneKind::Audio);
    vox.colour = Some(0x4ACB8B);
    vox.clips = vec![PreviewClip {
        name: "Vocal take 3".into(),
        start: beat(16.0),
        length: beat(8.0),
        audio: vec![AudioPart {
            path: r"C:\Made up\Vocal take 3.wav".into(),
            start: 0.0,
            length: beat(8.0),
            from: 0.0,
            seconds_per_beat: Some(0.5),
            whole_file_beats: None,
            reversed: false,
        }],
        ..PreviewClip::default()
    }];
    let mut cutoff = PreviewLane::new("Filter cutoff", LaneKind::Automation);
    cutoff.colour = Some(0xF2B33D);
    cutoff.clips = vec![PreviewClip {
        name: "Cutoff".into(),
        start: 0.0,
        length: beat(16.0),
        points: vec![
            PreviewPoint { beat: 0.0, value: 0.1 },
            PreviewPoint { beat: beat(6.0), value: 0.35 },
            PreviewPoint { beat: beat(8.0), value: 0.2 },
            PreviewPoint { beat: beat(15.5), value: 0.95 },
            PreviewPoint { beat: beat(16.0), value: 0.6 },
        ],
        ..PreviewClip::default()
    }];
    let playlist = Arrangement {
        view: "FL Studio playlist",
        tempo: 128.0,
        beats_per_bar: 4,
        lanes: vec![drums, kick, hats, bass, keys, vox, cutoff],
        markers: vec![
            PreviewMarker { beat: 0.0, name: "Intro".into() },
            PreviewMarker { beat: beat(16.0), name: "Drop".into() },
            PreviewMarker { beat: beat(24.0), name: "Break".into() },
        ],
        notes: Vec::new(),
    };
    let mut bitwig = playlist.clone();
    bitwig.view = "As it opens in Bitwig";
    Preview { views: vec![Arc::new(playlist), Arc::new(bitwig)], found: Arc::default() }
}
