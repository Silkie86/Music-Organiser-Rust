//! The Sounds tiles' shapes on the graphics card: every tile's background, edge, dot, waveform and liked bar on show,
//! drawn as one picture under the list (`TileGrid` in sounds.slint), which then shows only the tiles' words, hearts and
//! the playing tile's waveform. The window draws one picture much faster than the hundreds of shapes of a screen of
//! tiles. The shapes are where the window would draw them (each tile's parts at the places sounds.slint gives every
//! tile) and drawn as the window draws them: the waveform's lines as `waves` draws a Waveform's, softened as the
//! window's drawing library softens a path's. Where there's no card (the screenshots, an old driver, `MO_DRAW_CPU`),
//! while the painter is being set up, or once the card refused, the picture is empty and each tile draws its own.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use mo_app::log;
use mo_app::shapes::{Colour, MOST_PIXELS, Scene, Shape};
use mo_app::waves::{self as wave, WaveLine};
use slint::{ComponentHandle, Image, Model};

use super::{PAGE, Page, TILES, list};
use crate::canvas::{Canvas, Kind};
use crate::{AppWindow, Theme, TileGrid, TileGridAt, TileLook, gpu, waves};

/// How many sounds' waveform lines are kept, so scrolling doesn't read every waveform on show again in every frame
/// (a few kilobytes each). Past this they're all let go and read again as they show.
const KEEP_LINES: usize = 4096;

/// The waveform lines kept, for the sound list they're of: a new list starts again. Held by a `Weak`, which lets the
/// old list go but keeps its address from being given to a new one.
#[derive(Default)]
struct Lines {
    list: std::sync::Weak<list::SoundList>,
    sounds: HashMap<usize, Option<Rc<[WaveLine]>>>,
}

thread_local! {
    /// The painter on the graphics card.
    static CARD: RefCell<gpu::Painter<Canvas>> = const { RefCell::new(gpu::Painter::new()) };
    static LINES: RefCell<Lines> = RefCell::default();
    static APP: RefCell<Option<slint::Weak<AppWindow>>> = const { RefCell::new(None) };
}

/// Answers the list's asks for its picture.
pub(super) fn connect(app: &AppWindow) {
    APP.with(|a| *a.borrow_mut() = Some(app.as_weak()));
    let weak = app.as_weak();
    app.global::<TileGrid>().on_picture(move |at, look, _asked| {
        let Some(app) = weak.upgrade() else { return Image::default() };
        picture(&app, &at, &look).unwrap_or_default()
    });
}

/// The tiles changed (the list, a pick, what's playing, the scores): the picture is drawn again.
pub(super) fn changed() {
    let app = APP.with(|a| a.borrow().as_ref().and_then(slint::Weak::upgrade));
    if let Some(app) = app {
        ask_again(&app);
    }
}

/// A graphics card was handed over: the painter is set up on it, away from the window's thread, and then the picture
/// is drawn on it.
pub(super) fn graphics_ready(app: &AppWindow) {
    gpu::prepare(app, "Tiles", &CARD, painter, ask_again);
}

fn painter(gpu: &gpu::Gpu) -> Canvas {
    Canvas::new(&gpu.device, &gpu.queue, Kind::Picture)
}

/// The card went: the painter goes with it, now, while wgpu is still here.
pub(super) fn card_gone() {
    CARD.with(|c| c.borrow_mut().gone());
}

/// The card went and the window's still here: the tiles draw their own shapes until there's a card again.
pub(super) fn graphics_gone(app: &AppWindow) {
    ask_again_later(app.as_weak());
}

fn ask_again(app: &AppWindow) {
    let grid = app.global::<TileGrid>();
    grid.set_asked(grid.get_asked().wrapping_add(1));
}

/// Asks again at the event loop's next turn: not now, while the window may be working out what to show.
fn ask_again_later(app: slint::Weak<AppWindow>) {
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(app) = app.upgrade() {
            ask_again(&app);
        }
    });
}

/// The picture of the tiles on show, drawn on the card now; `None` where the tiles draw their own shapes.
fn picture(app: &AppWindow, at: &TileGridAt, look: &TileLook) -> Option<Image> {
    let (gpu, generation) = CARD.with(|c| c.borrow().card())?;
    if !CARD.with(|c| c.borrow().made_on(generation)) {
        // Set up away from the window's thread; the tiles draw their own shapes until it's ready.
        gpu::prepare(app, "Tiles", &CARD, painter, ask_again);
        return None;
    }
    let scale = at.scale;
    if !(scale.is_finite() && scale > 0.0) || !(at.width >= 1.0 && at.height >= 1.0) {
        return None;
    }
    let types: Vec<Colour> = app.global::<Theme>().get_sound_types().iter().map(colour).collect();
    let scene = PAGE.with(|p| {
        let p = p.borrow();
        (p.view == TILES).then(|| scene(at, look, &p, &types))
    })?;
    let size = scene.pixel_size(f64::from(scale));
    if size.0 >= MOST_PIXELS || size.1 >= MOST_PIXELS {
        return None;
    }
    let fringe = waves::fringe(scale);
    let drawn = CARD.with(|c| -> Result<Image, String> {
        let mut c = c.borrow_mut();
        let canvas = c.on(&gpu, generation, |gpu| {
            let _doing = crate::watchdog::doing("Tiles: set up the GPU painter");
            painter(gpu)
        })?;
        let _doing = crate::watchdog::doing("Tiles: draw on the GPU");
        let bytes = scene.pack(f64::from(scale));
        let texture = gpu::checked(&gpu.device, || canvas.draw(size, scale, scene.background, &bytes, fringe))?;
        Image::try_from(texture).map_err(|e| format!("Slint wouldn't show the picture: {e}"))
    });
    match drawn {
        Ok(image) => Some(image),
        Err(e) => {
            log::error("Tiles", &format!("The graphics card couldn't draw the tiles, so the window draws them: {e}"));
            CARD.with(|c| c.borrow_mut().refused(generation));
            ask_again_later(app.as_weak());
            None
        }
    }
}

fn colour(c: slint::Color) -> Colour {
    Colour { r: c.red(), g: c.green(), b: c.blue(), a: c.alpha() }
}

fn rgb(c: slint::Color) -> u32 {
    u32::from(c.red()) << 16 | u32::from(c.green()) << 8 | u32::from(c.blue())
}

/// What a tile shows, as far as its shapes go.
struct Tile {
    picked: bool,
    playing: bool,
    tint: Colour,
    /// How liked, from 0 to 1, or below 0 when it isn't known yet.
    liked: f32,
    /// Its waveform's lines and how many points across, if it has one.
    wave: Option<(Rc<[WaveLine]>, u32)>,
}

/// The shapes of the tiles on show, in points from the list's top left, as far past a whole pixel as it sits.
fn scene(at: &TileGridAt, look: &TileLook, p: &Page, types: &[Colour]) -> Scene {
    let columns = p.columns.max(1);
    let entries = p.entries();
    let grey = Colour::rgb(0x6E6E6E, 1.0);
    let tiles = shapes_of(at, columns, entries, |row| {
        let i = p.entry(row)?;
        let s = p.list.sound(i)?;
        let tint = usize::try_from(list::type_colour(&s.sound_type)).ok().and_then(|t| types.get(t)).copied();
        Some(Tile {
            picked: p.picked == Some(i),
            playing: p.playing.is_some_and(|(at, _)| at == i),
            tint: tint.unwrap_or(grey),
            liked: p.list.liked_share(i).map_or(-1.0, |l| l as f32),
            wave: lines_of(p, i, s),
        })
    });
    let (scale, width, height) = (f64::from(at.scale), f64::from(at.width), f64::from(at.height));
    // A pixel more each way, for drawing it as far past a whole pixel as the list sits.
    let mut scene = Scene::new(width + 1.0 / scale, height + 1.0 / scale, rgb(look.background));
    // The well the list is in (studio.slint's `Well`), drawn as the window draws an edged rectangle (see `draw_tile`).
    let (over_x, over_y) = (f64::from(at.over_x) / scale, f64::from(at.over_y) / scale);
    let (fill, edge) = (colour(look.well), Some(colour(look.edge)));
    scene.rounded((over_x + 0.5, over_y + 0.5, width - 1.0, height - 1.0), inner_radius(look.radius), fill, edge);
    for (x, y, tile) in tiles {
        draw_tile(&mut scene, at, look, (x, y), &tile);
    }
    scene
}

/// The tiles on show, with each one's top left in points from the picture's (as far past a whole pixel as the list
/// sits): `tile` says what a list row's tile shows.
fn shapes_of<T>(
    at: &TileGridAt,
    columns: usize,
    entries: usize,
    mut tile: impl FnMut(usize) -> Option<T>,
) -> Vec<(f64, f64, T)> {
    let (top, gap, height) = (f64::from(at.top), f64::from(at.gap), f64::from(at.tile_height));
    let step = height + gap;
    let (over_x, over_y) = (f64::from(at.over_x) / f64::from(at.scale), f64::from(at.over_y) / f64::from(at.scale));
    let mut out = Vec::new();
    if !(step > 0.0 && top.is_finite()) {
        return out;
    }
    // About the first row whose tiles reach into sight (any before it that don't are passed over below).
    let first = ((-top - gap - height) / step).floor().max(0.0) as usize;
    let rows = entries.div_ceil(columns);
    for row in first..rows {
        let y = top + gap + row as f64 * step;
        if y >= f64::from(at.height) {
            break;
        }
        if y + height <= 0.0 {
            continue;
        }
        for column in 0..columns {
            let i = row * columns + column;
            if i >= entries {
                break;
            }
            let x = gap + column as f64 * (f64::from(at.tile_width) + gap);
            if let Some(t) = tile(i) {
                out.push((x + over_x, y + over_y, t));
            }
        }
    }
    out
}

/// A sound's waveform lines, kept for the next frame.
fn lines_of(p: &Page, i: usize, s: &mo_sounds::Sound) -> Option<(Rc<[WaveLine]>, u32)> {
    let points = s.analysis.as_ref().map_or(0, |a| a.waveform.len());
    let points = u32::try_from(points).ok().filter(|n| *n > 0)?;
    let lines = LINES.with(|l| {
        let mut l = l.borrow_mut();
        if !std::ptr::eq(l.list.as_ptr(), std::sync::Arc::as_ptr(&p.list)) || l.sounds.len() >= KEEP_LINES {
            l.list = std::sync::Arc::downgrade(&p.list);
            l.sounds.clear();
        }
        l.sounds.entry(i).or_insert_with(|| wave::lines(&super::wave(s)).map(Rc::from)).clone()
    })?;
    Some((lines, points))
}

/// The corners' radius of the path the window strokes a 1-point edge along (half a point in from the rectangle's
/// own): half a point less round, but always a little round (Slint's `BorderRectLayout`).
fn inner_radius(radius: f32) -> f64 {
    (f64::from(radius).max(0.51) - 0.5).max(0.0)
}

/// One tile's shapes, as the window draws them (sounds.slint's `SoundTileView`): its background and edge, the dot, the
/// waveform (not while it plays: the tile shows that itself) and the liked bar. Under the mouse the tile draws them
/// itself too, over its shade, which covers these.
fn draw_tile(scene: &mut Scene, at: &TileGridAt, look: &TileLook, (x, y): (f64, f64), tile: &Tile) {
    let (w, h) = (f64::from(at.tile_width), f64::from(at.tile_height));
    let inside = f64::from(at.inside);
    // The window strokes a 1-point edge along a path half a point in, with corners half a point less round, over a fill
    // of that path (Slint's `BorderRectLayout`, for an edge that shows through nothing).
    let (fill, edge) = if tile.picked { (look.raised, look.accent) } else { (look.surface, look.edge) };
    scene.rounded((x + 0.5, y + 0.5, w - 1.0, h - 1.0), inner_radius(look.radius), colour(fill), Some(colour(edge)));
    let line_height = f64::from(at.name_height).max(10.0);
    scene.rounded((x + inside, y + inside + (line_height - 8.0) / 2.0, 8.0, 8.0), 4.0, tile.tint, None);
    let across = w - 2.0 * inside;
    if !tile.playing
        && let Some((lines, points)) = &tile.wave
    {
        let size = (across, f64::from(at.wave_height));
        let line = (across / f64::from(*points) * 0.6).max(1.0);
        let wave = wave::scene(lines, *points, size, (0.0, 0.0), line, tile.tint);
        let (left, top) = ((x + inside) as f32, (y + f64::from(at.wave_top)) as f32);
        for shape in wave.shapes {
            if let Shape::Line { from, to, width, colour } = shape {
                scene.shapes.push(Shape::Line {
                    from: (from.0 + left, from.1 + top),
                    to: (to.0 + left, to.1 + top),
                    width,
                    colour,
                });
            }
        }
    }
    if tile.liked >= 0.0 {
        let bar_y = y + h - inside - 3.0;
        scene.rounded((x + inside, bar_y, across, 3.0), 1.5, colour(look.hover), None);
        scene.rounded((x + inside, bar_y, across * f64::from(tile.liked.max(0.0)), 3.0), 1.5, colour(look.muted), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(top: f32, height: f32) -> TileGridAt {
        TileGridAt {
            top,
            width: 640.0,
            height,
            columns: 3,
            tile_width: 200.0,
            tile_height: 112.0,
            gap: 8.0,
            inside: 12.0,
            name_height: 19.0,
            wave_top: 58.0,
            wave_height: 35.0,
            over_x: 0.0,
            over_y: 0.0,
            scale: 1.0,
        }
    }

    #[test]
    fn the_tiles_on_show_are_where_the_list_puts_them() {
        let shown = shapes_of(&at(0.0, 240.0), 3, 10, Some);
        let places: Vec<(f64, f64, usize)> = shown;
        assert_eq!(
            places,
            [(8.0, 8.0, 0), (216.0, 8.0, 1), (424.0, 8.0, 2), (8.0, 128.0, 3), (216.0, 128.0, 4), (424.0, 128.0, 5),]
        );
    }

    #[test]
    fn scrolled_down_only_the_rows_in_sight_are_drawn() {
        // 130 points down: the first row ends at 120, so it's gone; the second row starts at -2.
        let shown = shapes_of(&at(-130.0, 100.0), 3, 10, Some);
        let rows: Vec<(f64, usize)> = shown.iter().map(|(_, y, i)| (*y, *i)).collect();
        assert_eq!(rows, [(-2.0, 3), (-2.0, 4), (-2.0, 5)]);
        // The last row holds only what's left.
        let shown = shapes_of(&at(-360.0, 300.0), 3, 10, Some);
        assert_eq!(shown.last().map(|s| s.2), Some(9));
    }

    #[test]
    fn the_picture_goes_as_far_past_a_whole_pixel_as_the_list() {
        let mut a = at(0.0, 100.0);
        a.over_x = 0.5;
        a.over_y = 0.25;
        a.scale = 1.25;
        let shown = shapes_of(&a, 3, 1, Some);
        assert_eq!(shown, [(8.4, 8.2, 0)]);
    }

    #[test]
    fn a_tile_is_drawn_as_the_window_draws_it() {
        let look = TileLook {
            radius: 4.0,
            background: slint::Color::from_rgb_u8(0x12, 0x12, 0x12),
            well: slint::Color::from_rgb_u8(0x0F, 0x0F, 0x0F),
            surface: slint::Color::from_rgb_u8(0x1D, 0x1D, 0x1D),
            raised: slint::Color::from_rgb_u8(0x25, 0x25, 0x25),
            edge: slint::Color::from_rgb_u8(0x0B, 0x0B, 0x0B),
            accent: slint::Color::from_rgb_u8(0x3D, 0xDC, 0x97),
            hover: slint::Color::from_rgb_u8(0x2A, 0x2A, 0x2A),
            muted: slint::Color::from_rgb_u8(0xA3, 0xA3, 0xA3),
        };
        let tint = Colour::rgb(0x4FB3FF, 1.0);
        let lines: Rc<[WaveLine]> = Rc::from(vec![WaveLine { x: 0.5, top: 0.0, bottom: 100.0 }]);
        let tile = Tile { picked: true, playing: false, tint, liked: 0.5, wave: Some((lines, 4)) };
        let mut scene = Scene::new(300.0, 200.0, 0);
        draw_tile(&mut scene, &at(0.0, 200.0), &look, (8.0, 8.0), &tile);
        let accent = Some(colour(look.accent));
        assert_eq!(
            scene.shapes[0],
            Shape::Rounded { x: 8.5, y: 8.5, w: 199.0, h: 111.0, radius: 3.5, fill: colour(look.raised), edge: accent }
        );
        assert_eq!(
            scene.shapes[1],
            Shape::Rounded { x: 20.0, y: 25.5, w: 8.0, h: 8.0, radius: 4.0, fill: tint, edge: None }
        );
        // The waveform is 176 across and 35 down, at 12 in and 58 down; its line is 176 / 4 * 0.6 = 26.4 wide.
        let Shape::Line { from, to, width, .. } = scene.shapes[2] else { panic!("{:?}", scene.shapes[2]) };
        assert!((width - 26.4).abs() < 1e-4, "{width}");
        let across = (176.0 - 26.4) / 4.0;
        assert!((from.0 - (20.0 + 13.2 + 0.5 * across)).abs() < 1e-3, "{from:?}");
        assert!((from.1 - (66.0 + 13.2)).abs() < 1e-3 && (to.1 - (66.0 + 13.2 + (35.0 - 26.4))).abs() < 1e-3, "{to:?}");
        // The liked bar, half full, at the bottom.
        assert_eq!(
            &scene.shapes[3..],
            [
                Shape::Rounded {
                    x: 20.0,
                    y: 105.0,
                    w: 176.0,
                    h: 3.0,
                    radius: 1.5,
                    fill: colour(look.hover),
                    edge: None
                },
                Shape::Rounded {
                    x: 20.0,
                    y: 105.0,
                    w: 88.0,
                    h: 3.0,
                    radius: 1.5,
                    fill: colour(look.muted),
                    edge: None
                },
            ]
        );
        // Playing, the tile shows its waveform itself.
        let tile = Tile { playing: true, liked: -1.0, ..tile };
        let mut scene = Scene::new(300.0, 200.0, 0);
        draw_tile(&mut scene, &at(0.0, 200.0), &look, (8.0, 8.0), &tile);
        assert_eq!(scene.shapes.len(), 2);
        assert!(scene.shapes.iter().all(|s| matches!(s, Shape::Rounded { .. })));
    }
}
