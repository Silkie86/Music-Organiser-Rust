//! The Sounds page's galaxy: every sound as a star on a map, sound-alikes close together (`mo_sounds::galaxy`). The
//! map is the C# app's saved one, brought up to date (or laid out afresh) on the low-priority pool; it's kept in
//! memory only, as sounds.db stays read-only until it's trusted. The graphics card draws the stars (`sky`), every
//! frame; without one they're drawn into a picture off the window's thread instead, newest ask first. The filters dim
//! the stars they leave out. Which part of the map is on show, the wheel and clicks versus drags are
//! `mo_app::galaxy_view`'s; the star under the mouse is found on a grid over the whole map (`mo_app::galaxy_grid`).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mo_app::background::{self, Newest};
use mo_app::galaxy_grid::{Crowd, Grid};
use mo_app::galaxy_look::{self, LightBy};
use mo_app::galaxy_names::{self as names, NO_KIND, Region};
use mo_app::galaxy_picture::{Drawn, Picture};
use mo_app::galaxy_view::{Hold, Moved, View, most_zoom_for};
use mo_app::log;
use mo_app::sounds as list;
use mo_app::{frame_timing, sweep_timing};
use mo_sounds::galaxy::{self, Placing, Step};
use mo_sounds::{SoundsDb, StarPoint};
use slint::{ComponentHandle, Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, Timer, TimerMode, VecModel};

use super::sky::{Shine, Sky};
use super::{FEED, HERE, PAGE, pick, play_picked, strings};
use crate::gpu;
use crate::{AppWindow, GalaxyLabel, SoundsData, SweptLine, Theme, TrailDot};

/// The Placed by choices, as the page names them, and what each places the stars by.
const PLACINGS: [(&str, Placing); 3] =
    [("Both", Placing::Both), ("Deep listening", Placing::Clap), ("Measurements", Placing::Own)];
/// How near the mouse (in the window's points) a star has to be to be the one under it.
const NEAR: f32 = 10.0;
/// How often a glide moves the view on: about 60 times a second.
const FRAME: Duration = Duration::from_millis(16);
/// The mini-map's size, in points, and how far in the view has to be before it shows (as C#).
const MINI: f32 = 132.0;
const MINI_FROM_ZOOM: f32 = 1.25;
/// While the view moves, pictures are drawn at no more than this many pixels per point, so they keep up; the sharp
/// one follows when it stops.
const MOVING_SCALE: f32 = 1.0;

/// The most sounds the sweep's trail and the Swept card hold (the C# app's 16).
const TRAIL: usize = 16;

/// Each star's sound type, for the names over the map: a place in `names` by the star's place in the list
/// ([`NO_KIND`] for none), and each type's colour.
#[derive(Default)]
pub(super) struct Kinds {
    of: Vec<u32>,
    names: Vec<String>,
    colours: Vec<i32>,
}

/// The map as it arrives: every star by its place in the list, each one's colour, and its type.
type Arrived = Option<(Vec<Option<StarPoint>>, Vec<i32>, Kinds)>;

/// The galaxy's part of the page.
pub(super) struct Galaxy {
    /// Where the sound library is, to read its saved map; `None` (the screenshots) lays out afresh.
    pub(super) db: Option<PathBuf>,
    placing: Placing,
    /// What makes a star bigger and brighter.
    light_by: LightBy,
    /// How lit each star is, worked out (off the window's thread) for one list and one Light by, and kept: a search
    /// only changes which stars are shown, so it doesn't rank half a million sounds again on every key.
    light: Option<Lit>,
    /// Every star's place on the map (0 to 1), by its place in the list; `None` until the map is in.
    stars: Option<Arc<Vec<Option<StarPoint>>>>,
    /// Each star's colour, as its place in `Theme.sound-types` (-1 grey), by its place in the list.
    colours: Option<Arc<Vec<i32>>>,
    /// The map being read or laid out, and what stops it.
    loading: Option<Arc<AtomicBool>>,
    note: String,
    /// The part of the map on show, and a press on it.
    view: View,
    /// Pixels per point.
    scale: f32,
    /// The press going on was cancelled (the window lost the mouse), so it isn't a click.
    cancelled: bool,
    /// Counts the pictures asked for, so an older one that arrives late is dropped.
    asked: u64,
    /// The shown stars on a grid over the map, to find the one under the mouse and count the ones on the view.
    grid: Option<Arc<Grid>>,
    /// The stars as the graphics card reads them, kept to send again to a new card, with how many at their end are lit
    /// by the measure (`galaxy_look::pack`).
    packed: Option<Arc<(Vec<u8>, u32)>>,
    /// What the grid (and the card's stars) were last asked for, how many times they've been asked, and which ask
    /// the ones in hand are from.
    set_for: Option<SetKey>,
    set_asked: u64,
    set_got: u64,
    /// The view the picture on show was drawn for, and its number.
    drawn: Option<View>,
    drawn_id: u64,
    /// How many stars are on the map.
    count: usize,
    /// A left-button sweep going on.
    sweep: Sweep,
    /// The mouse is over the map.
    inside: bool,
    /// Each star's type, and the neighbourhoods named over the map for the stars on show, with which list of shown
    /// stars they were found for (so they're found again when the filters change).
    kinds: Option<Arc<Kinds>>,
    regions: Arc<Vec<Region>>,
    regions_for: Option<Arc<Vec<usize>>>,
    /// The sounds swept (and clicked) lately, newest first, by place in the list, and the one Back and Next are on.
    trail: Vec<usize>,
    trail_at: usize,
}

impl Default for Galaxy {
    fn default() -> Galaxy {
        Galaxy {
            db: None,
            placing: Placing::Both,
            light_by: LightBy::default(),
            light: None,
            stars: None,
            colours: None,
            loading: None,
            note: String::new(),
            view: View::default(),
            scale: 1.0,
            cancelled: false,
            asked: 0,
            grid: None,
            packed: None,
            set_for: None,
            set_asked: 0,
            set_got: 0,
            drawn: None,
            drawn_id: 0,
            count: 0,
            sweep: Sweep::default(),
            inside: false,
            kinds: None,
            regions: Arc::default(),
            regions_for: None,
            trail: Vec::new(),
            trail_at: 0,
        }
    }
}

/// How lit each star is (see [`galaxy_look::light`]) for one list of sounds and one Light by, and for the stars shown
/// when it came: how much of them has the measure and their order by it.
#[derive(Clone)]
struct Lit {
    list: Arc<list::SoundList>,
    by: LightBy,
    light: Arc<Vec<f32>>,
    /// How much of what's on show has the measure (`galaxy_look::lit_share`).
    share: f32,
    /// The shown stars with the measure, least lit first (`galaxy_look::lit_order`).
    order: Arc<Vec<u32>>,
}

impl Lit {
    fn of(&self, list: &Arc<list::SoundList>, by: LightBy) -> bool {
        Arc::ptr_eq(&self.list, list) && self.by == by
    }
}

impl Galaxy {
    /// The kept light for the page's list and the Light by chosen now, if it's been worked out.
    fn light_for(&self, list: &Arc<list::SoundList>) -> Option<&Lit> {
        self.light.as_ref().filter(|l| l.of(list, self.light_by))
    }
}

/// What the stars were packed for: the map, its colours, the stars the filters let through and the types' colours.
#[derive(Clone)]
struct SetKey {
    stars: Arc<Vec<Option<StarPoint>>>,
    colours: Arc<Vec<i32>>,
    shown: Arc<Vec<usize>>,
    palette: Vec<[u8; 3]>,
    /// The sounds (their likes, uses and lengths light the stars) and what lights them.
    list: Arc<list::SoundList>,
    light_by: LightBy,
    /// The light for `list` and `light_by`, when it's been worked out already.
    light: Option<Arc<Vec<f32>>>,
}

impl SetKey {
    fn same(&self, o: &SetKey) -> bool {
        Arc::ptr_eq(&self.stars, &o.stars)
            && Arc::ptr_eq(&self.colours, &o.colours)
            && Arc::ptr_eq(&self.shown, &o.shown)
            && self.palette == o.palette
            && Arc::ptr_eq(&self.list, &o.list)
            && self.light_by == o.light_by
    }
}

/// An ask to sort the stars onto the grid and, for the graphics card, pack them.
struct SetAsk {
    key: SetKey,
    version: u64,
    for_card: bool,
}

/// The grid and the packed stars, and which ask they answer, with the light they were packed with.
struct Packed {
    version: u64,
    grid: Grid,
    /// The stars for the card, and how many at their end are lit by the measure.
    bytes: Option<(Vec<u8>, u32)>,
    light: Lit,
}

impl SetAsk {
    fn work(self) -> Packed {
        let k = &self.key;
        // All of it on the app's low-priority pool, spread over its cores (rule 21).
        let (grid, (light, share, order, bytes)) = background::on_pool(|| {
            rayon::join(
                || Grid::new(&k.stars, &k.shown),
                || {
                    // Ranked only when the list or Light by has changed; a new search reuses it.
                    let light = k.light.clone().unwrap_or_else(|| Arc::new(galaxy_look::light(&k.list, k.light_by)));
                    let share = galaxy_look::lit_share(&k.stars, &k.shown, &light);
                    let order = Arc::new(galaxy_look::lit_order(&k.stars, &k.shown, &light));
                    let bytes = self
                        .for_card
                        .then(|| galaxy_look::pack(&k.stars, &k.colours, &k.palette, &k.shown, &light, &order));
                    (light, share, order, bytes)
                },
            )
        });
        let light = Lit { list: k.list.clone(), by: k.light_by, light, share, order };
        Packed { version: self.version, grid, bytes, light }
    }
}

/// The galaxy's painter on the graphics card, and which packed stars it holds.
struct OnCard {
    sky: Sky,
    stars: u64,
}

thread_local! {
    static DRAWING: RefCell<Option<Newest<Picture>>> = const { RefCell::new(None) };
    static PACKING: RefCell<Option<Newest<SetAsk>>> = const { RefCell::new(None) };
    static MINI_DRAWING: RefCell<Option<Newest<Picture>>> = const { RefCell::new(None) };
    static CARD: RefCell<gpu::Painter<OnCard>> = const { RefCell::new(gpu::Painter::new()) };
    /// Moves a glide on a frame at a time, and when it last did.
    static FLIGHT: Timer = Timer::default();
    static LAST_FRAME: RefCell<Option<Instant>> = const { RefCell::new(None) };
}

/// How often the picture is drawn again while the stars twinkle (how much they do, and the rules for when, are
/// `galaxy_look`'s: `TWINKLE`, `twinkle_amount`, `twinkles` and `beat`).
const TWINKLE_EVERY: Duration = Duration::from_millis(80);
/// The twinkle stops this long after the galaxy was last drawn for anything else (a move, a filter) or the mouse last
/// moved over it, so a window left open on the galaxy doesn't keep the graphics card busy all day.
const TWINKLE_FOR: Duration = Duration::from_secs(30);

thread_local! {
    /// Draws the galaxy again now and then while its stars twinkle.
    static TWINKLING: Timer = Timer::default();
    /// The twinkle's clock.
    static TWINKLE_CLOCK: Instant = Instant::now();
    /// Whether the last picture drawn on the card had stars drawn as stars, the ones that twinkle
    /// (`galaxy_look::twinkles`: far out with nothing lit, none do).
    static TWINKLE_SHOWS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Until when the stars twinkle (see [`TWINKLE_FOR`]).
    static TWINKLE_UNTIL: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
    /// Since when the view has been still (no button held, no glide), for the twinkle to come back in.
    static STILL_SINCE: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
}

/// The graphics card to draw the galaxy on, when there's one it can draw on: not for the screenshots (drawn on the
/// processor), and not one that refused to.
fn usable_card() -> Option<(gpu::Gpu, u64)> {
    if HERE.with(|h| *h.borrow()) {
        return None;
    }
    CARD.with(|c| c.borrow().card())
}

/// How many shown stars are on the view and how crowded they are, as the grid finds them (until the grid comes, all of
/// them, spread over the view).
fn crowd(g: &Galaxy) -> Crowd {
    match &g.grid {
        Some(grid) => {
            let (l, t, r, b) = galaxy_look::on_view(&g.view);
            grid.crowd(l, t, r, b)
        }
        None => Crowd::even(&g.view, g.count),
    }
}

/// How much the stars twinkle now: not at all while a button is held or the view glides (`still` is false), so a
/// sweep or a glide never has them flicker; once it's still, the twinkle comes back in (`galaxy_look::twinkle_amount`).
fn twinkle_now(still: bool) -> f32 {
    STILL_SINCE.with(|since| {
        if !still {
            since.set(None);
            return 0.0;
        }
        let from = since.get().unwrap_or_else(Instant::now);
        since.set(Some(from));
        galaxy_look::twinkle_amount(Some(from.elapsed()))
    })
}

/// Keeps the stars twinkling on the graphics card while the galaxy is on show and its picture has stars drawn as stars.
/// Each beat draws only the stars again (the card keeps the painted picture under them). While a button is held or the
/// view glides the beat leaves the picture as it is (a glide draws it anyway), and once the view is still the twinkle
/// comes back in from nothing. Once the galaxy is off show (another page, the window minimised), nothing in its picture
/// twinkles, or [`TWINKLE_FOR`] after it was last drawn for anything else or the mouse last moved over it, the beat
/// stops (`galaxy_look::beat`).
fn keep_twinkling(app: &AppWindow) {
    TWINKLE_UNTIL.with(|u| u.set(Some(Instant::now() + TWINKLE_FOR)));
    if TWINKLING.with(Timer::running) {
        return;
    }
    let weak = app.as_weak();
    TWINKLING.with(|t| {
        t.start(TimerMode::Repeated, TWINKLE_EVERY, move || {
            let Some(app) = weak.upgrade() else { return };
            // On show: the Sounds page in front, its galaxy with stars, the window not minimised, and drawn lately.
            let lately = TWINKLE_UNTIL.with(std::cell::Cell::get).is_some_and(|u| Instant::now() < u);
            let in_front = crate::place() == Some(mo_app::places::Place::Sounds) && !app.window().is_minimized();
            let (on_show, moving) = PAGE.with(|p| {
                let p = p.borrow();
                let g = &p.galaxy;
                (
                    p.view == super::GALAXY && g.stars.is_some() && in_front && lately,
                    g.view.pressed() || g.view.flying(),
                )
            });
            let card = usable_card();
            let twinkles = TWINKLE_SHOWS.with(std::cell::Cell::get);
            match galaxy_look::beat(on_show && card.is_some(), twinkles, moving) {
                galaxy_look::Beat::Stop => {
                    TWINKLING.with(Timer::stop);
                    STILL_SINCE.with(|s| s.set(None));
                    return;
                }
                // The stars hold still; once the view is still again the twinkle comes back in from nothing.
                galaxy_look::Beat::Hold => {
                    STILL_SINCE.with(|s| s.set(None));
                    return;
                }
                galaxy_look::Beat::Draw => {}
            }
            let Some((gpu, generation)) = card else { return };
            let _doing = crate::watchdog::doing("Galaxy: twinkle");
            if let Err(e) = draw_on_card(&app, &gpu, generation, false) {
                log::warning("Sounds", &format!("The graphics card couldn't draw the galaxy's twinkle: {e}"));
                TWINKLING.with(Timer::stop);
            }
        });
    });
}

/// Starts moving a glide on, if it isn't already.
fn fly(app: &AppWindow) {
    if FLIGHT.with(Timer::running) {
        return;
    }
    LAST_FRAME.with(|l| *l.borrow_mut() = Some(Instant::now()));
    let weak = app.as_weak();
    FLIGHT.with(|t| {
        t.start(TimerMode::Repeated, FRAME, move || {
            let Some(app) = weak.upgrade() else { return };
            let _doing = crate::watchdog::doing(frame_timing::GLIDING);
            let now = Instant::now();
            let dt = LAST_FRAME.with(|l| l.borrow_mut().replace(now).map_or(FRAME, |was| now - was));
            let going = PAGE.with(|p| p.borrow_mut().galaxy.view.glide(dt.min(Duration::from_millis(100))));
            // The picture on show moves at once; a new one is asked for (newest wins, so asks don't pile up), sharp
            // once the glide has stopped.
            draw(&app);
            if !going {
                FLIGHT.with(Timer::stop);
                hover(&app);
            }
        });
    });
}

/// Wires the galaxy's choices and the mouse on the map.
pub(super) fn wire(app: &AppWindow) {
    let data = app.global::<SoundsData>();
    data.set_galaxy_placings(strings(PLACINGS.map(|p| p.0)));
    data.set_galaxy_placing(PLACINGS[0].0.into());
    let weak = app.as_weak();
    data.on_galaxy_place_by(move |v| {
        let Some(app) = weak.upgrade() else { return };
        let Some(&(_, placing)) = PLACINGS.iter().find(|p| p.0 == v.as_str()) else { return };
        PAGE.with(|p| p.borrow_mut().galaxy.placing = placing);
        app.global::<SoundsData>().set_galaxy_placing(v);
        load(&app, false);
    });
    data.set_galaxy_lights(strings(LightBy::ALL.map(LightBy::name)));
    data.set_galaxy_light(LightBy::default().name().into());
    let weak = app.as_weak();
    data.on_galaxy_light_by(move |v| {
        let Some(app) = weak.upgrade() else { return };
        let Some(by) = LightBy::from_name(v.as_str()) else { return };
        PAGE.with(|p| p.borrow_mut().galaxy.light_by = by);
        app.global::<SoundsData>().set_galaxy_light(v);
        draw(&app);
    });
    let weak = app.as_weak();
    data.on_galaxy_lay_out(move || {
        if let Some(app) = weak.upgrade() {
            load(&app, true);
        }
    });
    let weak = app.as_weak();
    data.on_galaxy_reset(move || {
        let Some(app) = weak.upgrade() else { return };
        PAGE.with(|p| p.borrow_mut().galaxy.view.fly_home());
        fly(&app);
    });
    let weak = app.as_weak();
    data.on_galaxy_resized(move |w, h| {
        let Some(app) = weak.upgrade() else { return };
        let scale = app.window().scale_factor();
        PAGE.with(|p| {
            let g = &mut p.borrow_mut().galaxy;
            (g.view.width, g.view.height, g.scale) = (w, h, scale);
        });
        draw(&app);
    });
    let weak = app.as_weak();
    data.on_galaxy_wheel(move |notches| {
        let Some(app) = weak.upgrade() else { return };
        PAGE.with(|p| p.borrow_mut().galaxy.view.fly_at_mouse(notches));
        fly(&app);
    });
    let weak = app.as_weak();
    data.on_galaxy_down(move |x, y, pan| {
        let Some(app) = weak.upgrade() else { return };
        // A second button pressed while one is held changes nothing: the first press carries on.
        if PAGE.with(|p| p.borrow().galaxy.view.pressed()) {
            return;
        }
        PAGE.with(|p| {
            let g = &mut p.borrow_mut().galaxy;
            g.view.press(x, y, if pan { Hold::Pan } else { Hold::Sweep });
            g.cancelled = false;
            g.sweep = Sweep::default();
        });
        let data = app.global::<SoundsData>();
        data.set_galaxy_grabbing(pan);
        data.set_galaxy_hover_shown(false);
    });
    let weak = app.as_weak();
    data.on_galaxy_move(move |x, y| {
        let Some(app) = weak.upgrade() else { return };
        let _doing = crate::watchdog::doing("Galaxy: mouse move");
        let moved = PAGE.with(|p| {
            let g = &mut p.borrow_mut().galaxy;
            g.inside = true;
            g.view.moved(x, y)
        });
        let data = app.global::<SoundsData>();
        match moved {
            Moved::Panned => {
                let _doing = crate::watchdog::doing(frame_timing::DRAGGING);
                data.set_galaxy_hover_text("".into());
                data.set_galaxy_hover_shown(false);
                draw(&app);
            }
            Moved::Still => {}
            Moved::Swept(fx, fy) => swept(&app, (fx, fy), (x, y)),
            Moved::Hover => {
                data.set_galaxy_mouse_x(x);
                data.set_galaxy_mouse_y(y);
                hover(&app);
            }
        }
    });
    data.on_galaxy_cancelled(|| PAGE.with(|p| p.borrow_mut().galaxy.cancelled = true));
    let weak = app.as_weak();
    data.on_galaxy_up(move |side| {
        let Some(app) = weak.upgrade() else { return };
        // Only the button that started the press ends it (a cancelled press ends whichever it was).
        let other = PAGE.with(|p| {
            let g = &p.borrow().galaxy;
            let held = g.view.holding();
            held.is_none() || (!g.cancelled && held != Some(if side { Hold::Pan } else { Hold::Sweep }))
        });
        if other {
            return;
        }
        app.global::<SoundsData>().set_galaxy_grabbing(false);
        let (click, swept, panned) = PAGE.with(|p| {
            let g = &mut p.borrow_mut().galaxy;
            let cancelled = std::mem::take(&mut g.cancelled);
            let swept = g.view.sweeping() && g.sweep.played.is_some();
            let panned = g.view.panning();
            g.sweep = Sweep::default();
            (g.view.release(cancelled), swept, panned)
        });
        SWEEP_TIMER.with(Timer::stop);
        // The drag drew quick pictures: the sharp one now.
        if panned {
            draw(&app);
        }
        hover(&app);
        // Let go after sweeping: the sound stops with the mouse.
        if swept {
            super::stop(&app);
        }
        let Some(at) = click.and_then(|(x, y)| star_at(x, y)) else { return };
        on_trail(&app, at, false);
        pick(&app, at);
        if PAGE.with(|p| p.borrow().autoplay) {
            play_picked(&app, 0.0);
        }
    });
    // A double-click plays the star; with autoplay on, its first click already did.
    let weak = app.as_weak();
    data.on_galaxy_double(move || {
        let Some(app) = weak.upgrade() else { return };
        let (autoplay, (x, y)) = PAGE.with(|p| {
            let p = p.borrow();
            (p.autoplay, p.galaxy.view.mouse)
        });
        if !autoplay && let Some(at) = star_at(x, y) {
            pick(&app, at);
            play_picked(&app, 0.0);
        }
    });
    let weak = app.as_weak();
    data.on_galaxy_mini(move |fx, fy| {
        let Some(app) = weak.upgrade() else { return };
        PAGE.with(|p| {
            let g = &mut p.borrow_mut().galaxy;
            let (x, y) = mini_view().to_map(fx.clamp(0.0, 1.0) * MINI, fy.clamp(0.0, 1.0) * MINI);
            let zoom = g.view.zoom;
            g.view.fly_to(x.clamp(0.0, 1.0), y.clamp(0.0, 1.0), zoom);
        });
        fly(&app);
    });
    let weak = app.as_weak();
    data.on_galaxy_label(move |at| {
        let Some(app) = weak.upgrade() else { return };
        let flew = PAGE.with(|p| {
            let g = &mut p.borrow_mut().galaxy;
            let area = g.regions.get(usize::try_from(at).ok()?)?.area;
            g.view.fly_into(area);
            Some(())
        });
        if flew.is_some() {
            fly(&app);
        }
    });
    let weak = app.as_weak();
    data.on_galaxy_swept_go(move |i| {
        if let Some(app) = weak.upgrade() {
            go_to_swept(&app, usize::try_from(i).unwrap_or(0));
        }
    });
    let weak = app.as_weak();
    data.on_galaxy_swept_step(move |by| {
        let Some(app) = weak.upgrade() else { return };
        let to = PAGE.with(|p| {
            let g = &p.borrow().galaxy;
            let last = g.trail.len().checked_sub(1)?;
            Some((g.trail_at as i64 + i64::from(by)).clamp(0, last as i64) as usize)
        });
        if let Some(to) = to {
            go_to_swept(&app, to);
        }
    });
    let weak = app.as_weak();
    data.on_galaxy_swept_clear(move || {
        let Some(app) = weak.upgrade() else { return };
        PAGE.with(|p| {
            let g = &mut p.borrow_mut().galaxy;
            g.trail.clear();
            g.trail_at = 0;
        });
        show_trail(&app);
    });
    let weak = app.as_weak();
    data.on_galaxy_gone(move || {
        PAGE.with(|p| p.borrow_mut().galaxy.inside = false);
        if let Some(app) = weak.upgrade() {
            let data = app.global::<SoundsData>();
            data.set_galaxy_hover_text("".into());
            data.set_galaxy_hover_shown(false);
        }
    });
}

/// How long each star of a sweep plays before the next one may start, so a sweep over a crowded part isn't cut to
/// clicks. The C# app's 90 ms was most of the wait between the mouse reaching a star and hearing it, and skipped many
/// of the stars a quick sweep passes (Solomon, 2026-11-04): 40 ms still lets each sound start cleanly.
const SWEEP_GAP: Duration = Duration::from_millis(40);
/// How often a star waiting its turn looks whether its turn has come: a small part of [`SWEEP_GAP`], so it doesn't
/// add much to the wait.
const SWEEP_LOOK: Duration = Duration::from_millis(5);
/// How much of a swept sound is read and played: its start is what a sweep hears, and a whole loop would take a
/// moment to read.
const SWEEP_SECONDS: f64 = 4.0;
/// How far apart (in points) the mouse's path is looked along for stars.
const SWEEP_STEP: f32 = 3.0;

/// A sweep going on: when the last star started and which it was, and the star the mouse reached since, waiting
/// for its turn. Only the newest waits, so the sound keeps up with the mouse.
#[derive(Default)]
pub(super) struct Sweep {
    played: Option<(usize, Instant)>,
    waiting: Option<usize>,
    /// When the mouse last moved in this sweep, to tell how fast it's going.
    moved: Option<Instant>,
    /// Where the pointer is held, on the star waiting its turn, when sweeps hold the pointer.
    held: Option<(f32, f32)>,
}

/// How far ahead of the mouse a sweep's next sounds are read (the C# app's 40 ms), at most how far in points, how
/// many, and how much of each.
const AHEAD: Duration = Duration::from_millis(40);
const AHEAD_MOST: f32 = 200.0;
const AHEAD_SOUNDS: usize = 4;
const AHEAD_BYTES: usize = 128 * 1024;

thread_local! {
    static SWEEP_TIMER: Timer = Timer::default();
    /// Whether a sweep holds the pointer to the star waiting its turn (Settings › General), so none is passed unheard.
    static HOLD: Cell<bool> = const { Cell::new(false) };
    /// Reads the start of the sounds a sweep is about to reach, newest ask first.
    static READ_AHEAD: RefCell<Option<Newest<Vec<String>>>> = const { RefCell::new(None) };
}

/// Reads the start of each file, so the disk has it ready when the sweep reaches it (the system keeps what was read
/// lately), and the sound plays the moment the mouse gets there.
fn warm(paths: Vec<String>) {
    use std::io::Read;
    let mut buffer = vec![0u8; AHEAD_BYTES];
    for path in paths {
        if let Ok(mut f) = std::fs::File::open(&path)
            && f.read(&mut buffer).is_ok()
        {
            sweep_timing::read_ahead(&path);
        }
    }
}

/// The mouse is sweeping from `from` to `to` at this speed: the sounds a little further along its way are read ahead.
fn read_ahead(from: (f32, f32), to: (f32, f32), dt: Duration, passed: &[usize]) {
    if HERE.with(|h| *h.borrow()) || dt.is_zero() {
        return;
    }
    let k = AHEAD.as_secs_f32() / dt.as_secs_f32();
    let (mut dx, mut dy) = ((to.0 - from.0) * k, (to.1 - from.1) * k);
    let far = dx.hypot(dy);
    if far < 1.0 {
        return;
    }
    if far > AHEAD_MOST {
        (dx, dy) = (dx * AHEAD_MOST / far, dy * AHEAD_MOST / far);
    }
    let steps = (far.min(AHEAD_MOST) / SWEEP_STEP).ceil() as usize;
    let paths = PAGE.with(|p| {
        let p = p.borrow();
        let g = &p.galaxy;
        let playing = g.sweep.played.map(|(at, _)| at);
        let mut next: Vec<usize> = Vec::new();
        for i in 1..=steps {
            let t = i as f32 / steps as f32;
            if let Some(at) = star_at_in(g, to.0 + dx * t, to.1 + dy * t)
                && !next.contains(&at)
                && !passed.contains(&at)
                && playing != Some(at)
            {
                next.push(at);
                if next.len() == AHEAD_SOUNDS {
                    break;
                }
            }
        }
        next.iter().filter_map(|&at| Some(p.list.sound(at)?.path.clone())).collect::<Vec<String>>()
    });
    if paths.is_empty() {
        return;
    }
    READ_AHEAD.with(|w| {
        let mut w = w.borrow_mut();
        if w.is_none() {
            match Newest::start("Galaxy read ahead", warm) {
                Ok(worker) => *w = Some(worker),
                Err(e) => log::warning("Sounds", &format!("Couldn't start reading the sweep ahead: {e}")),
            }
        }
        if let Some(worker) = w.as_ref() {
            worker.ask(paths);
        }
    });
}

/// Whether a Galaxy sweep holds the pointer to the star waiting its turn.
pub(super) fn hold_sweeps(on: bool) {
    HOLD.with(|h| h.set(on));
}

/// The mouse swept from `from` to `to`: every star along the way, not just where it landed (a quick sweep moves many
/// points between two moves), plays in turn. When sweeps hold the pointer, a star that has to wait its turn keeps
/// the pointer on it until it plays, and the stars past it aren't reached yet.
fn swept(app: &AppWindow, from: (f32, f32), to: (f32, f32)) {
    let _doing = crate::watchdog::doing(frame_timing::SWEEPING);
    let hold = HOLD.with(Cell::get);
    // Held on a star still waiting: the pointer goes back there, and the move reaches nothing new.
    if hold
        && let Some(at) = PAGE.with(|p| {
            let s = &p.borrow().galaxy.sweep;
            s.held.filter(|_| s.waiting.is_some())
        })
    {
        hold_at(app, at, to);
        return;
    }
    let steps = ((to.0 - from.0).hypot(to.1 - from.1) / SWEEP_STEP).ceil().max(1.0) as usize;
    let mut passed: Vec<(usize, (f32, f32))> = Vec::new();
    // Whether the first star passed is the one under the last mouse point.
    let mut from_a_star = false;
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let point = (from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t);
        if let Some(at) = star_at(point.0, point.1)
            && passed.last().map(|&(a, _)| a) != Some(at)
        {
            from_a_star |= i == 0;
            passed.push((at, point));
        }
    }
    let data = app.global::<SoundsData>();
    data.set_galaxy_mouse_x(to.0);
    data.set_galaxy_mouse_y(to.1);
    let now = Instant::now();
    let was = PAGE.with(|p| p.borrow_mut().galaxy.sweep.moved.replace(now));
    if let Some(was) = was {
        let stars: Vec<usize> = passed.iter().map(|&(at, _)| at).collect();
        read_ahead(from, to, now - was, &stars);
    }
    // Timed from here: Slint doesn't say when the move happened, so how late the window heard of it isn't known. The
    // star under the last mouse point was reached on an earlier move (as C#'s HitPath, which starts past it), so its
    // wait isn't started again; on the sweep's first move it's reached now.
    let skip = usize::from(was.is_some() && from_a_star);
    for (i, &(at, point)) in passed.iter().enumerate() {
        if i >= skip {
            PAGE.with(|p| {
                if let Some(s) = p.borrow().list.sound(at) {
                    sweep_timing::reached(&s.path, None);
                }
            });
        }
        if !reached(app, at) && hold {
            PAGE.with(|p| p.borrow_mut().galaxy.sweep.held = Some(point));
            hold_at(app, point, to);
            return;
        }
    }
}

/// Holds the pointer at `at` (on the view, in points) while it's at `now`: the pointer is put back there, and the
/// sweep goes on from there.
fn hold_at(app: &AppWindow, at: (f32, f32), now: (f32, f32)) {
    PAGE.with(|p| p.borrow_mut().galaxy.view.mouse = at);
    let data = app.global::<SoundsData>();
    data.set_galaxy_mouse_x(at.0);
    data.set_galaxy_mouse_y(at.1);
    // Putting it where it already is would only send another move to the same place.
    if (now.0 - at.0).abs() + (now.1 - at.1).abs() < 0.5 {
        return;
    }
    use slint::winit_030::WinitWindowAccessor;
    use slint::winit_030::winit::dpi::PhysicalPosition;
    let scale = app.window().scale_factor();
    let x = f64::from((data.get_galaxy_sky_x() + at.0) * scale);
    let y = f64::from((data.get_galaxy_sky_y() + at.1) * scale);
    let put = app.window().with_winit_window(|w| w.set_cursor_position(PhysicalPosition::new(x, y)));
    if let Some(Err(e)) = put {
        // Once is enough: the sweep goes on without holding.
        log::warning("Sounds", &format!("Couldn't hold the pointer on a swept star, so sweeps won't hold it: {e}"));
        HOLD.with(|h| h.set(false));
    }
}

/// The sweep reached a star: it plays now if the last one has had its moment, otherwise it waits its turn. Whether
/// it didn't have to wait (it plays now, or it's the one playing).
fn reached(app: &AppWindow, at: usize) -> bool {
    let now = Instant::now();
    // None: it's the one playing, and nothing changes.
    let play = PAGE.with(|p| {
        let g = &mut p.borrow_mut().galaxy;
        let last = g.sweep.played;
        if last.is_some_and(|(was, _)| was == at) && g.sweep.waiting.is_none() {
            return None;
        }
        if last.is_none_or(|(_, when)| now.duration_since(when) >= SWEEP_GAP) {
            g.sweep.waiting = None;
            g.sweep.played = Some((at, now));
            Some(true)
        } else {
            g.sweep.waiting = Some(at);
            Some(false)
        }
    });
    match play {
        None => return true,
        Some(true) => {
            sweep_play(app, at);
            return true;
        }
        Some(false) => {}
    }
    let weak = app.as_weak();
    SWEEP_TIMER.with(|t| {
        if !t.running() {
            t.start(TimerMode::Repeated, SWEEP_LOOK, move || {
                let Some(app) = weak.upgrade() else { return };
                let due = PAGE.with(|p| {
                    let g = &mut p.borrow_mut().galaxy;
                    let waiting = g.sweep.waiting?;
                    let ready = g.sweep.played.is_none_or(|(_, when)| when.elapsed() >= SWEEP_GAP);
                    ready.then(|| {
                        g.sweep.waiting = None;
                        g.sweep.held = None;
                        g.sweep.played = Some((waiting, Instant::now()));
                        waiting
                    })
                });
                match due {
                    Some(at) => sweep_play(&app, at),
                    None if PAGE.with(|p| p.borrow().galaxy.sweep.waiting.is_none()) => SWEEP_TIMER.with(Timer::stop),
                    None => {}
                }
            });
        }
    });
    false
}

/// Plays a star the sweep reached, and names it beside the mouse. It isn't picked: a sweep is for listening along.
/// It goes on the trail.
fn sweep_play(app: &AppWindow, at: usize) {
    let _doing = crate::watchdog::doing("Galaxy: play a swept sound");
    let name = PAGE.with(|p| {
        let p = p.borrow();
        let s = p.list.sound(at)?;
        sweep_timing::asked(&s.path);
        Some(format!("{}  ·  {}", s.name(), s.sound_type))
    });
    app.global::<SoundsData>().set_galaxy_hover_text(name.unwrap_or_default().into());
    on_trail(app, at, true);
    super::play_sound(app, at, 0.0, SWEEP_SECONDS);
}

/// Puts a sound on the trail, newest first (as the C# app: a swept one moves to the front; a clicked one already on
/// it stays where it is), and Back and Next start from it.
fn on_trail(app: &AppWindow, at: usize, swept: bool) {
    PAGE.with(|p| {
        let g = &mut p.borrow_mut().galaxy;
        match g.trail.iter().position(|&t| t == at) {
            Some(i) if !swept => g.trail_at = i,
            found => {
                if let Some(i) = found {
                    g.trail.remove(i);
                }
                g.trail.insert(0, at);
                g.trail.truncate(TRAIL);
                g.trail_at = 0;
            }
        }
    });
    show_trail(app);
}

/// Goes to the `i`th sound on the trail (0 the newest): picks it, plays it and glides the map to it, keeping the zoom.
fn go_to_swept(app: &AppWindow, i: usize) {
    let found = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let g = &mut p.galaxy;
        let at = *g.trail.get(i)?;
        g.trail_at = i;
        if let Some(star) = g.stars.as_ref().and_then(|s| *s.get(at)?) {
            let zoom = g.view.zoom;
            g.view.fly_to(star.x, star.y, zoom);
        }
        Some(at)
    });
    let Some(at) = found else { return };
    show_trail(app);
    pick(app, at);
    play_picked(app, 0.0);
    fly(app);
}

/// The trail on the map (a line through the swept stars and a dot on each, fainter with age) and on the Swept card.
fn show_trail(app: &AppWindow) {
    let _doing = crate::watchdog::doing("Galaxy: show the trail");
    let (path, dots, lines) = PAGE.with(|p| {
        let p = p.borrow();
        let g = &p.galaxy;
        let Some(stars) = g.stars.as_ref() else { return (String::new(), Vec::new(), Vec::new()) };
        let n = g.trail.len().max(1) as f32;
        let on_view: Vec<(f32, f32, f32)> = g
            .trail
            .iter()
            .enumerate()
            .filter_map(|(i, &at)| {
                let star = (*stars.get(at)?)?;
                let (x, y) = g.view.to_view(star.x, star.y, 1.0);
                Some((x, y, 1.0 - i as f32 / n))
            })
            .collect();
        let mut path = String::new();
        if on_view.len() > 1 {
            for (i, (x, y, _)) in on_view.iter().enumerate() {
                path.push_str(&format!("{} {x:.1} {y:.1} ", if i == 0 { "M" } else { "L" }));
            }
        }
        let near = |&(x, y, _): &(f32, f32, f32)| {
            x > -10.0 && y > -10.0 && x < g.view.width + 10.0 && y < g.view.height + 10.0
        };
        let dots: Vec<TrailDot> =
            on_view.iter().filter(|d| near(d)).map(|&(x, y, fade)| TrailDot { x, y, fade }).collect();
        let lines: Vec<SweptLine> = g
            .trail
            .iter()
            .enumerate()
            .filter_map(|(i, &at)| {
                let s = p.list.sound(at)?;
                Some(SweptLine {
                    name: s.name().into(),
                    kind: s.sound_type.as_str().into(),
                    colour: list::type_colour(&s.sound_type),
                    current: i == g.trail_at,
                })
            })
            .collect();
        (path, dots, lines)
    });
    let data = app.global::<SoundsData>();
    data.set_galaxy_trail(path.into());
    data.set_galaxy_trail_dots(ModelRc::new(VecModel::from(dots)));
    data.set_galaxy_swept(ModelRc::new(VecModel::from(lines)));
}

/// The names over the map where the view is now; the neighbourhoods are found again (on the pool) when the stars on
/// show have changed.
fn show_names(app: &AppWindow) {
    let _doing = crate::watchdog::doing("Galaxy: show the names");
    let ask = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let shown = p.shown.clone();
        let g = &mut p.galaxy;
        let fresh = g.regions_for.as_ref().is_some_and(|f| Arc::ptr_eq(f, &shown));
        if fresh || g.stars.is_none() || g.kinds.is_none() {
            return None;
        }
        g.regions_for = Some(shown.clone());
        Some((g.stars.clone()?, g.kinds.clone()?, shown))
    });
    if let Some((stars, kinds, shown)) = ask {
        let find = move || background::on_pool(|| names::find(&stars, &kinds.of, &kinds.names, &kinds.colours, &shown));
        let take = |app: &AppWindow, found: Vec<Region>, shown: &Arc<Vec<usize>>| {
            let _doing = crate::watchdog::doing("Galaxy: take the names found");
            let current = PAGE.with(|p| {
                let g = &mut p.borrow_mut().galaxy;
                let current = g.regions_for.as_ref().is_some_and(|f| Arc::ptr_eq(f, shown));
                if current {
                    g.regions = Arc::new(found);
                }
                current
            });
            if current {
                place_names(app);
            }
        };
        let for_shown = PAGE.with(|p| p.borrow().shown.clone());
        if HERE.with(|h| *h.borrow()) {
            take(app, find(), &for_shown);
        } else {
            let weak = app.as_weak();
            let started = background::spawn("Galaxy names", move || {
                let found = find();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(app) = weak.upgrade() {
                        take(&app, found, &for_shown);
                    }
                });
            });
            if let Err(e) = started {
                log::warning("Sounds", &format!("Couldn't start naming the galaxy: {e}"));
            }
        }
    }
    place_names(app);
}

/// Puts the names found where they are on the view now.
fn place_names(app: &AppWindow) {
    let _doing = crate::watchdog::doing("Galaxy: place the names (property updates)");
    let labels = PAGE.with(|p| {
        let g = &p.borrow().galaxy;
        // About a capital's width with its letter spacing, and the line with its padding (the names are Type.meta,
        // 13 px, bold, in capitals).
        names::place(&g.regions, &g.view, |name| (name.chars().count() as f32 * 10.5 + 16.0, 22.0))
    });
    let labels: Vec<GalaxyLabel> = labels
        .into_iter()
        .map(|l| GalaxyLabel {
            region: l.region as i32,
            name: l.name.to_uppercase().into(),
            x: l.x,
            y: l.y,
            colour: l.colour,
            fade: l.fade,
        })
        .collect();
    app.global::<SoundsData>().set_galaxy_labels(ModelRc::new(VecModel::from(labels)));
}

/// Names the star under the mouse and rings it, so it's plain which one a click picks.
fn hover(app: &AppWindow) {
    let _doing = crate::watchdog::doing("Galaxy: hover");
    let data = app.global::<SoundsData>();
    // While a button is held, the ring stays away and a sweep's name stays as it is.
    if PAGE.with(|p| p.borrow().galaxy.view.pressed()) {
        data.set_galaxy_hover_shown(false);
        return;
    }
    // Someone's looking: the stars keep twinkling while the mouse moves over them (when the card drew some as stars).
    if TWINKLE_SHOWS.with(std::cell::Cell::get) && PAGE.with(|p| p.borrow().galaxy.inside) {
        keep_twinkling(app);
    }
    let found = PAGE.with(|p| {
        let p = p.borrow();
        let g = &p.galaxy;
        if g.view.flying() || !g.inside {
            return None;
        }
        let (x, y) = g.view.mouse;
        let at = star_at_in(g, x, y)?;
        let star = (*g.stars.as_ref()?.get(at)?)?;
        let name = p.list.sound(at).map(|s| format!("{}  ·  {}", s.name(), s.sound_type)).unwrap_or_default();
        Some((name, g.view.to_view(star.x, star.y, 1.0)))
    });
    data.set_galaxy_hover_shown(found.is_some());
    match found {
        Some((name, (x, y))) => {
            data.set_galaxy_hover_text(name.into());
            data.set_galaxy_hover_x(x);
            data.set_galaxy_hover_y(y);
        }
        None => data.set_galaxy_hover_text("".into()),
    }
}

/// The shown star under (`x`, `y`) on the view, in points.
fn star_at(x: f32, y: f32) -> Option<usize> {
    PAGE.with(|p| star_at_in(&p.borrow().galaxy, x, y))
}

fn star_at_in(g: &Galaxy, x: f32, y: f32) -> Option<usize> {
    let (mx, my) = g.view.to_map(x, y);
    g.grid.as_ref()?.nearest(mx, my, NEAR / g.view.unit())
}

/// A new library: its stars are placed afresh (they're kept by place in the list).
pub(super) fn forget(g: &mut Galaxy) {
    if let Some(old) = g.loading.take() {
        old.store(true, Ordering::Relaxed);
    }
    g.stars = None;
    g.colours = None;
    g.kinds = None;
    g.regions = Arc::default();
    g.regions_for = None;
    g.trail.clear();
    g.trail_at = 0;
    g.view.most = most_zoom_for(0);
    g.grid = None;
    g.packed = None;
    g.light = None;
    // The old library's stars on the card aren't drawn over the new one's.
    CARD.with(|c| {
        if let Some(on) = c.borrow_mut().made() {
            on.sky.clear_stars();
            on.stars = 0;
        }
    });
    g.set_for = None;
    // Stars still being packed for the old library are too old to use, and so is a mini-map still being drawn of it
    // (it's shown only if it's of the newest stars, `set_got`).
    g.set_asked += 1;
    g.set_got = g.set_asked;
    g.drawn = None;
    // A picture of the old library still on its way is too old to show.
    g.drawn_id = g.asked;
    g.count = 0;
    g.note = String::new();
}

/// The page's list was read again with the same sounds in the same places: the stars stay, and what was worked out
/// from the old list (how lit each star is, what the stars were packed for) is let go, and the old list with it. It's
/// worked out again for the new one when the galaxy's next drawn.
pub(super) fn list_changed(g: &mut Galaxy) {
    g.light = None;
    g.set_for = None;
    // Stars still being packed for the old list would bring it back (with how lit they are): too old to use. The ones
    // in use stay until new ones come.
    g.set_asked += 1;
    g.set_got = g.set_asked;
}

/// The page's list was read again with sounds new or gone (`to`: each old place's new one, `None` for a sound that's
/// gone): the star a sweep last played and the one waiting its turn move with their sounds. One that's gone isn't
/// waited for, and the pointer isn't held to it.
pub(super) fn places_moved(g: &mut Galaxy, to: impl Fn(usize) -> Option<usize>) {
    let s = &mut g.sweep;
    s.played = s.played.and_then(|(at, when)| Some((to(at)?, when)));
    s.waiting = s.waiting.and_then(&to);
    if s.waiting.is_none() {
        s.held = None;
    }
}

/// The view came on show (or a new library did while it was): reads the map the first time.
pub(super) fn shown(app: &AppWindow) {
    let _doing = crate::watchdog::doing("Galaxy: came on show");
    let start = PAGE.with(|p| {
        let p = p.borrow();
        p.galaxy.stars.is_none() && p.galaxy.loading.is_none()
    });
    if start {
        // The old library's picture goes; the new map is on its way.
        let data = app.global::<SoundsData>();
        data.set_galaxy_image(Image::default());
        data.set_galaxy_mini_image(Image::default());
        data.set_galaxy_picked_shown(false);
        load(app, false);
    } else {
        draw(app);
    }
}

/// The fingerprints arrived: the map can be read if the galaxy is on show and was waiting.
pub(super) fn index_known(app: &AppWindow) {
    if PAGE.with(|p| p.borrow().view == super::GALAXY) {
        shown(app);
    }
}

/// Reads the saved map for the Placed by choice and brings it up to date, or lays it out afresh when `fresh`, on the
/// low-priority pool, with a card showing how far it's got. A newer ask stops an older one.
fn load(app: &AppWindow, fresh: bool) {
    let _doing = crate::watchdog::doing("Galaxy: ask for the map");
    let ask = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let index = p.index.clone();
        let list = p.list.clone();
        let places = p.places.clone();
        let g = &mut p.galaxy;
        if let Some(old) = g.loading.take() {
            old.store(true, Ordering::Relaxed);
        }
        let Some(index) = index.filter(|_| !list.is_empty()) else {
            g.note = "Listening to your sounds first: their fingerprints are still being read.".to_owned();
            return None;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        g.loading = Some(cancel.clone());
        g.note = if fresh { "Laying out the galaxy afresh…" } else { "Reading the galaxy…" }.to_owned();
        Some((index, list, places, g.db.clone(), g.placing, cancel))
    });
    show_note(app);
    let Some((index, list, places, db, placing, cancel)) = ask else { return };
    let mine = cancel.clone();
    let weak = app.as_weak();
    let here = HERE.with(|h| *h.borrow());
    let feed = FEED.with(|f| f.borrow().clone()).filter(|_| !here);
    let work = move || {
        let space = galaxy::space(placing, &index);
        let saved = match &db {
            Some(path) => match SoundsDb::open_read_only(path).and_then(|db| db.map(space)) {
                Ok(map) => map,
                Err(e) => {
                    log::warning("Sounds", &format!("Couldn't read the saved galaxy ({space}), so it's laid out: {e}"));
                    HashMap::new()
                }
            },
            None => HashMap::new(),
        };
        let job = feed.map(|f| Mutex::new(f.start("Galaxy", "Bringing the map up to date")));
        let progress = |s: Step| {
            if let Some(job) = &job
                && let Ok(job) = job.lock()
            {
                job.progress(s.done as f32 / 4.0, Some(s.what));
            }
        };
        let updated = background::on_pool(|| galaxy::update(&index, space, saved, fresh, &progress, &cancel));
        let job = job.and_then(|j| j.into_inner().ok());
        let stars = updated.map(|u| {
            let mut stars = vec![None; list.len()];
            for (id, point) in u.map {
                if let Some(&at) = places.get(&id) {
                    stars[at] = Some(point);
                }
            }
            (stars, u.replace)
        });
        // Each star's colour, worked out once rather than on every picture.
        let colours: Vec<i32> = list.sounds().iter().map(|s| list::type_colour(&s.sound_type)).collect();
        let kinds = kinds_of(&list);
        match (&stars, job) {
            (Some((_, true)), Some(job)) => job.done("Laid out afresh (kept in memory until sounds.db is trusted)"),
            (Some(_), Some(job)) => job.done("Up to date"),
            (None, Some(job)) => job.done("Stopped"),
            _ => {}
        }
        stars.map(|s| (s.0, colours, kinds))
    };
    let arrive = move |app: &AppWindow, stars: Arrived, cancel: &Arc<AtomicBool>| {
        let _doing = crate::watchdog::doing("Galaxy: the map arrived");
        let current = PAGE.with(|p| {
            let mut p = p.borrow_mut();
            let g = &mut p.galaxy;
            if !g.loading.as_ref().is_some_and(|c| Arc::ptr_eq(c, cancel)) {
                return false;
            }
            g.loading = None;
            if let Some((stars, colours, kinds)) = stars {
                let n = stars.iter().filter(|s| s.is_some()).count();
                g.note = if n == 0 {
                    "No sounds on the map yet: the analysis hasn't heard any.".to_owned()
                } else {
                    String::new()
                };
                g.stars = Some(Arc::new(stars));
                g.colours = Some(Arc::new(colours));
                g.kinds = Some(Arc::new(kinds));
                g.regions_for = None;
                g.count = n;
                g.view.most = most_zoom_for(n);
                g.view.zoom = g.view.zoom.min(g.view.most);
            }
            true
        });
        if current {
            show_note(app);
            draw(app);
        }
    };
    if here {
        let stars = work();
        arrive(app, stars, &mine);
        return;
    }
    let c = mine;
    let started = background::spawn("Galaxy", move || {
        let stars = work();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(app) = weak.upgrade() {
                arrive(&app, stars, &c);
            }
        });
    });
    if let Err(e) = started {
        log::error("Sounds", &format!("Couldn't start the galaxy: {e}"));
    }
}

/// Each sound's type as a number, and the types' names and colours, for the names over the map. A sound with no type,
/// or "Unknown", names nothing.
fn kinds_of(list: &list::SoundList) -> Kinds {
    let mut kinds = Kinds::default();
    let mut ids: HashMap<&str, u32> = HashMap::new();
    for s in list.sounds() {
        let t = s.sound_type.as_str();
        if t.is_empty() || t == mo_sounds::types::UNKNOWN {
            kinds.of.push(NO_KIND);
            continue;
        }
        let id = *ids.entry(t).or_insert_with(|| {
            kinds.names.push(t.to_owned());
            kinds.colours.push(list::type_colour(t));
            (kinds.names.len() - 1) as u32
        });
        kinds.of.push(id);
    }
    kinds
}

fn show_note(app: &AppWindow) {
    let _doing = crate::watchdog::doing("Galaxy: show the note");
    let data = app.global::<SoundsData>();
    PAGE.with(|p| {
        let g = &p.borrow().galaxy;
        data.set_galaxy_note(g.note.clone().into());
        data.set_galaxy_busy(g.loading.is_some());
    });
}

/// Draws the stars again (the map, the view, the filters or the window's size changed): on the graphics card at
/// once, or without one off the window's thread.
pub(super) fn draw(app: &AppWindow) {
    let _doing = crate::watchdog::doing("Galaxy: draw");
    let on_show = PAGE.with(|p| {
        let p = p.borrow();
        p.view == super::GALAXY && p.galaxy.stars.is_some()
    });
    if !on_show {
        show_picked(app);
        return;
    }
    let types = app.global::<Theme>().get_sound_types();
    let mut palette: Vec<[u8; 3]> = types.iter().map(|c| [c.red(), c.green(), c.blue()]).collect();
    palette.push([110, 110, 110]);
    let card = usable_card();
    pack(app, &palette, card.is_some());
    if let Some((gpu, generation)) = &card {
        let (gpu, generation) = (gpu, *generation);
        match draw_on_card(app, gpu, generation, true) {
            Ok(true) => {
                if TWINKLE_SHOWS.with(std::cell::Cell::get) {
                    keep_twinkling(app);
                }
                return;
            }
            // The stars aren't on the card yet: the picture on show stays until they are.
            Ok(false) => {
                place_picture(app);
                show_picked(app);
                return;
            }
            Err(e) => {
                log::error(
                    "Sounds",
                    &format!("The graphics card couldn't draw the galaxy, so the processor does: {e}"),
                );
                CARD.with(|c| c.borrow_mut().refused(generation));
                // The processor's picture doesn't twinkle.
                TWINKLE_SHOWS.with(|t| t.set(false));
            }
        }
    }
    let ask = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let shown = p.shown.clone();
        let list = p.list.clone();
        let g = &mut p.galaxy;
        let stars = g.stars.clone()?;
        let colours = g.colours.clone()?;
        if g.view.width < 1.0 || g.view.height < 1.0 {
            return None;
        }
        // Until the light for a new list or Light by has been worked out, the stars are drawn alike; the packing
        // that works it out asks for the picture again.
        let lit = g.light_for(&list);
        let light = lit.map(|l| l.light.clone()).unwrap_or_default();
        let order = lit.map(|l| l.order.clone()).unwrap_or_default();
        let crowd = crowd(g);
        g.asked += 1;
        let moving = g.view.flying() || g.view.panning();
        let scale = if moving { g.scale.min(MOVING_SCALE) } else { g.scale };
        Some(Picture {
            id: g.asked,
            stars,
            count: g.count,
            colours,
            shown,
            light,
            order,
            palette,
            view: g.view,
            scale,
            crowd,
            glows: true,
        })
    });
    let Some(ask) = ask else {
        show_picked(app);
        return;
    };
    place_picture(app);
    if HERE.with(|h| *h.borrow()) {
        let (d, px) = {
            let _doing = crate::watchdog::doing("Galaxy: CPU picture");
            to_image(ask.draw())
        };
        drawn(app, d, px);
        return;
    }
    DRAWING.with(|w| {
        let mut w = w.borrow_mut();
        if w.is_none() {
            let weak = app.as_weak();
            match Newest::start("Galaxy picture", move |ask: Picture| {
                let (d, px) = to_image(background::on_pool(|| ask.draw()));
                let weak = weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(app) = weak.upgrade() {
                        drawn(&app, d, px);
                    }
                });
            }) {
                Ok(worker) => *w = Some(worker),
                Err(e) => log::error("Sounds", &format!("Couldn't start the galaxy's drawing thread: {e}")),
            }
        }
        if let Some(worker) = w.as_ref() {
            worker.ask(ask);
        }
    });
}

/// Sorts the shown stars onto the grid (and packs them for the card) again when the map, the filters or the colours
/// have changed, off the window's thread. Until they come, the last ones stay in use.
fn pack(app: &AppWindow, palette: &[[u8; 3]], for_card: bool) {
    let _doing = crate::watchdog::doing("Galaxy: ask for the stars to be sorted");
    let ask = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let shown = p.shown.clone();
        let list = p.list.clone();
        let g = &mut p.galaxy;
        let light = g.light_for(&list).map(|l| l.light.clone());
        let key = SetKey {
            stars: g.stars.clone()?,
            colours: g.colours.clone()?,
            shown,
            palette: palette.to_vec(),
            list,
            light_by: g.light_by,
            light,
        };
        // Packed already, unless the card has come since and the stars weren't packed for it.
        let fresh = g.set_for.as_ref().is_some_and(|k| k.same(&key));
        if fresh && (!for_card || g.packed.is_some() || g.set_got < g.set_asked) {
            return None;
        }
        g.set_asked += 1;
        g.set_for = Some(key.clone());
        Some(SetAsk { key, version: g.set_asked, for_card })
    });
    let Some(ask) = ask else { return };
    if HERE.with(|h| *h.borrow()) {
        packed(app, ask.work());
        return;
    }
    PACKING.with(|w| {
        let mut w = w.borrow_mut();
        if w.is_none() {
            let weak = app.as_weak();
            match Newest::start("Galaxy stars", move |ask: SetAsk| {
                let done = ask.work();
                let weak = weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(app) = weak.upgrade() {
                        packed(&app, done);
                    }
                });
            }) {
                Ok(worker) => *w = Some(worker),
                Err(e) => log::error("Sounds", &format!("Couldn't start sorting the galaxy's stars: {e}")),
            }
        }
        if let Some(worker) = w.as_ref() {
            worker.ask(ask);
        }
    });
}

/// The grid (and the packed stars) came: newer than the ones in use, they're used, and the galaxy is drawn again.
fn packed(app: &AppWindow, done: Packed) {
    let _doing = crate::watchdog::doing("Galaxy: take the sorted stars");
    let newer = PAGE.with(|p| {
        let g = &mut p.borrow_mut().galaxy;
        if done.version <= g.set_got || done.version > g.set_asked {
            return false;
        }
        g.set_got = done.version;
        g.grid = Some(Arc::new(done.grid));
        g.packed = done.bytes.map(Arc::new);
        g.light = Some(done.light);
        true
    });
    if newer {
        draw(app);
        hover(app);
        draw_mini(app);
    }
}

/// The whole map in miniature for the mini-map, as it is with the filters now; drawn off the window's thread.
fn draw_mini(app: &AppWindow) {
    let _doing = crate::watchdog::doing("Galaxy: ask for the mini-map");
    let types = app.global::<Theme>().get_sound_types();
    let mut palette: Vec<[u8; 3]> = types.iter().map(|c| [c.red(), c.green(), c.blue()]).collect();
    palette.push([110, 110, 110]);
    let scale = app.window().scale_factor();
    let ask = PAGE.with(|p| {
        let p = p.borrow();
        let g = &p.galaxy;
        let key = g.set_for.as_ref()?;
        Some(Picture {
            id: g.set_got,
            stars: key.stars.clone(),
            count: g.count,
            colours: key.colours.clone(),
            shown: key.shown.clone(),
            // It only shows the map's shape, and where the view is on it: plain dots, all alike (Light by would dim
            // nearly all of them, the unlit ones).
            light: Arc::default(),
            order: Arc::default(),
            palette,
            view: mini_view(),
            scale,
            // The whole map is on show.
            crowd: Crowd::even(&mini_view(), key.shown.len()),
            glows: false,
        })
    });
    let Some(ask) = ask else { return };
    let show = |app: &AppWindow, d: Drawn, px: SharedPixelBuffer<Rgba8Pixel>| {
        let _doing = crate::watchdog::doing("Galaxy: show the mini-map picture");
        // Only the newest map's miniature.
        if PAGE.with(|p| p.borrow().galaxy.set_got) == d.id {
            app.global::<SoundsData>().set_galaxy_mini_image(Image::from_rgba8_premultiplied(px));
        }
    };
    if HERE.with(|h| *h.borrow()) {
        let (d, px) = to_image(ask.draw());
        show(app, d, px);
        return;
    }
    MINI_DRAWING.with(|w| {
        let mut w = w.borrow_mut();
        if w.is_none() {
            let weak = app.as_weak();
            match Newest::start("Galaxy mini-map", move |ask: Picture| {
                let (d, px) = to_image(background::on_pool(|| ask.draw()));
                let weak = weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(app) = weak.upgrade() {
                        show(&app, d, px);
                    }
                });
            }) {
                Ok(worker) => *w = Some(worker),
                Err(e) => log::warning("Sounds", &format!("Couldn't start drawing the mini-map: {e}")),
            }
        }
        if let Some(worker) = w.as_ref() {
            worker.ask(ask);
        }
    });
}

/// The mini-map's view: the whole map, at its own size.
fn mini_view() -> View {
    let mut v = View::default();
    (v.width, v.height) = (MINI, MINI);
    v
}

/// Shows the mini-map once the view has flown in (and there's room for it), with a box round the part on show.
fn show_mini(app: &AppWindow) {
    let _doing = crate::watchdog::doing("Galaxy: show the mini-map");
    let mini = PAGE.with(|p| {
        let g = &p.borrow().galaxy;
        let v = &g.view;
        if v.zoom <= MINI_FROM_ZOOM || v.width < 420.0 || v.height < 320.0 || g.count == 0 {
            return None;
        }
        let m = mini_view();
        let (l, t, r, b) = galaxy_look::on_view(v);
        let corner = |x: f32, y: f32| {
            let (px, py) = m.to_view(x, y, 1.0);
            ((px / MINI).clamp(0.0, 1.0), (py / MINI).clamp(0.0, 1.0))
        };
        let ((x0, y0), (x1, y1)) = (corner(l, t), corner(r, b));
        let on_view = g.grid.as_ref().map(|grid| grid.count_in(l, t, r, b));
        let title = match on_view {
            Some(n) => format!("×{:.0}  ·  {} in view", v.zoom, mo_app::songs::thousands(n)),
            None => format!("×{:.0}", v.zoom),
        };
        Some((title, x0, y0, x1 - x0, y1 - y0))
    });
    let data = app.global::<SoundsData>();
    data.set_galaxy_mini_shown(mini.is_some());
    if let Some((title, x, y, w, h)) = mini {
        data.set_galaxy_mini_title(title.into());
        data.set_galaxy_mini_x(x);
        data.set_galaxy_mini_y(y);
        data.set_galaxy_mini_width(w);
        data.set_galaxy_mini_height(h);
    }
}

/// The card went: the painter goes with it, now.
pub(super) fn painter_gone() {
    CARD.with(|c| c.borrow_mut().gone());
    TWINKLE_SHOWS.with(|t| t.set(false));
}

/// The card went: the picture made on it isn't shown any more (it's drawn again, on the next card or the processor,
/// when the galaxy next is).
pub(super) fn card_gone(app: &AppWindow) {
    let weak = app.as_weak();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(app) = weak.upgrade() {
            app.global::<SoundsData>().set_galaxy_image(Image::default());
        }
        PAGE.with(|p| {
            let g = &mut p.borrow_mut().galaxy;
            g.drawn = None;
            g.asked += 1;
            g.drawn_id = g.asked;
        });
    });
}

/// Draws the galaxy on the card, now: `false` when its stars aren't there yet. `whole`: what's over the picture is
/// placed again too (not for a twinkle, where only the stars' brightness changes).
fn draw_on_card(app: &AppWindow, gpu: &gpu::Gpu, generation: u64, whole: bool) -> Result<bool, String> {
    let ask = PAGE.with(|p| {
        let g = &p.borrow().galaxy;
        // Lit as the stars in hand were packed, even while newer ones (another Light by, say) are on their way: the
        // stars on the card are always drawn the way they were lit.
        let share = g.light.as_ref().map_or(0.0, |l| l.share);
        let shine = Shine {
            light_on: share > 0.0,
            // Wrapped every hour, so after days open it still has the precision a smooth twinkle needs.
            time: (TWINKLE_CLOCK.with(Instant::elapsed).as_secs_f64() % 3600.0) as f32,
            twinkle: twinkle_now(!g.view.pressed() && !g.view.flying()),
        };
        (g.view, g.scale, galaxy_look::look(&g.view, crowd(g), share), shine, g.packed.clone(), g.set_got)
    });
    let (view, scale, look, shine, packed, version) = ask;
    let lit = packed.as_ref().map_or(0, |p| p.1);
    TWINKLE_SHOWS.with(|t| t.set(galaxy_look::twinkles(&look, shine.light_on, lit)));
    if view.width < 1.0 || view.height < 1.0 {
        return Ok(false);
    }
    // The well the map sits in, so the picture matches what's round it.
    let bg = app.global::<Theme>().get_well();
    let background = [bg.red(), bg.green(), bg.blue()].map(|c| f32::from(c) / 255.0);
    let texture = CARD.with(|c| -> Result<Option<_>, String> {
        let mut c = c.borrow_mut();
        let on = c.on(gpu, generation, |gpu| {
            // The first time, the card builds the shader and its steps, which can take a while.
            let _doing = crate::watchdog::doing("Galaxy: set up the GPU painter");
            OnCard { sky: Sky::new(&gpu.device, &gpu.queue), stars: 0 }
        })?;
        if on.stars != version
            && let Some(packed) = &packed
        {
            let _doing = crate::watchdog::doing("Galaxy: upload the stars to the GPU");
            let (bytes, lit) = &**packed;
            gpu::checked(&gpu.device, || on.sky.set_stars(bytes, *lit))?;
            on.stars = version;
        }
        if !on.sky.has_stars() {
            return Ok(None);
        }
        let _doing = crate::watchdog::doing("Galaxy: draw on the GPU");
        gpu::checked(&gpu.device, || on.sky.draw(&view, scale, &look, shine, background)).map(Some)
    })?;
    let Some(texture) = texture else { return Ok(false) };
    let _doing = crate::watchdog::doing("Galaxy: show the GPU picture (property updates)");
    let image = Image::try_from(texture).map_err(|e| format!("Slint wouldn't show the picture: {e}"))?;
    PAGE.with(|p| {
        let g = &mut p.borrow_mut().galaxy;
        // A picture drawn on the processor that's still on its way is older than this one.
        g.asked += 1;
        g.drawn_id = g.asked;
        g.drawn = Some(view);
    });
    app.global::<SoundsData>().set_galaxy_image(image);
    if whole {
        place_picture(app);
        show_picked(app);
    }
    Ok(true)
}

/// The picture as the window shows it (copied, off the window's thread), and what else came with it.
fn to_image(mut d: Drawn) -> (Drawn, SharedPixelBuffer<Rgba8Pixel>) {
    let mut px = SharedPixelBuffer::<Rgba8Pixel>::new(d.width as u32, d.height as u32);
    for (to, from) in px.make_mut_slice().iter_mut().zip(&d.pixels) {
        *to = Rgba8Pixel { r: from.r, g: from.g, b: from.b, a: from.a };
    }
    d.pixels = Vec::new();
    (d, px)
}

/// A picture came. Any newer than the one on show is shown, even when a newer one still is on its way: while the wheel
/// keeps turning, each picture that's done is placed where its stars are now, instead of the first one stretching on.
fn drawn(app: &AppWindow, d: Drawn, px: SharedPixelBuffer<Rgba8Pixel>) {
    let _doing = crate::watchdog::doing("Galaxy: show the CPU picture");
    let current = PAGE.with(|p| {
        let g = &mut p.borrow_mut().galaxy;
        if d.id <= g.drawn_id || d.id > g.asked {
            return false;
        }
        g.drawn_id = d.id;
        g.drawn = Some(d.view);
        true
    });
    if current {
        app.global::<SoundsData>().set_galaxy_image(Image::from_rgba8_premultiplied(px));
        place_picture(app);
        show_picked(app);
        // The stars may have moved under the mouse (a new filter, size or map): the ring follows what a click picks.
        hover(app);
    }
}

/// Moves and stretches the last picture to where its stars are on the view now, at once, so the wheel and a drag
/// answer straight away; the sharp picture for the new view replaces it when it's drawn.
fn place_picture(app: &AppWindow) {
    let _doing = crate::watchdog::doing("Galaxy: move the picture (property updates)");
    let place = PAGE.with(|p| {
        let g = &p.borrow().galaxy;
        g.drawn.map(|drawn| g.view.place_of(&drawn))
    });
    let (x, y, w, h) = place.unwrap_or((0.0, 0.0, 0.0, 0.0));
    let data = app.global::<SoundsData>();
    data.set_galaxy_image_x(x);
    data.set_galaxy_image_y(y);
    data.set_galaxy_image_width(w);
    data.set_galaxy_image_height(h);
    // What's drawn over the picture moves with it.
    show_trail(app);
    show_names(app);
    show_mini(app);
}

/// The ring round the picked star, where it is on the view now.
pub(super) fn show_picked(app: &AppWindow) {
    let data = app.global::<SoundsData>();
    let at = PAGE.with(|p| {
        let p = p.borrow();
        let g = &p.galaxy;
        let star = (*g.stars.as_ref()?.get(p.picked?)?)?;
        let (x, y) = g.view.to_view(star.x, star.y, 1.0);
        (x >= 0.0 && y >= 0.0 && x <= g.view.width && y <= g.view.height).then_some((x, y))
    });
    data.set_galaxy_picked_shown(at.is_some());
    if let Some((x, y)) = at {
        data.set_galaxy_picked_x(x);
        data.set_galaxy_picked_y(y);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stars_packed_for_a_list_read_again_are_dropped() {
        // A pack asked for (3) while the one in use is 2.
        let mut g = Galaxy { set_asked: 3, set_got: 2, ..Galaxy::default() };
        list_changed(&mut g);
        assert!(g.set_for.is_none() && g.light.is_none());
        // The pack still on its way is 3: [`packed`] takes only one newer than `set_got` and no newer than asked.
        assert!(3 <= g.set_got && g.set_got == g.set_asked, "{} {}", g.set_got, g.set_asked);
    }

    #[test]
    fn a_sweeps_stars_move_with_their_sounds() {
        // The sound at 0 is gone, 1 is now at 0 and 2 at 3.
        let moved = [None, Some(0), Some(3)];
        let to = |at: usize| moved.get(at).copied().flatten();
        let when = Instant::now();
        let mut g = Galaxy::default();
        g.sweep.played = Some((1, when));
        g.sweep.waiting = Some(2);
        g.sweep.held = Some((5.0, 6.0));
        places_moved(&mut g, to);
        assert_eq!(g.sweep.played, Some((0, when)));
        assert_eq!(g.sweep.waiting, Some(3));
        assert_eq!(g.sweep.held, Some((5.0, 6.0)));
        // The one waiting is gone (and so is the one played): nothing's waited for, and the pointer isn't held.
        g.sweep.played = Some((0, when));
        g.sweep.waiting = Some(0);
        places_moved(&mut g, to);
        assert_eq!((g.sweep.played, g.sweep.waiting, g.sweep.held), (None, None, None));
    }
}
