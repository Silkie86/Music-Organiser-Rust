//! The Sounds page's galaxy: every sound as a star on a map, sound-alikes close together (`mo_sounds::galaxy`). The
//! map is the C# app's saved one, brought up to date (or laid out afresh) on the low-priority pool; it's kept in
//! memory only, as sounds.db stays read-only until it's trusted. The stars are drawn into a picture off the window's
//! thread, newest ask first, so 450,000 of them cost one image; the filters dim the stars they leave out. Which part
//! of the map is on show, the wheel and clicks versus drags are `mo_app::galaxy_view`'s.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use mo_app::background::{self, Newest};
use mo_app::galaxy_picture::{Drawn, Hits, Picture};
use mo_app::galaxy_view::{Moved, View};
use mo_app::log;
use mo_app::sounds as list;
use mo_sounds::galaxy::{self, Placing, Step};
use mo_sounds::{SoundsDb, StarPoint};
use slint::{ComponentHandle, Image, Model, Rgba8Pixel, SharedPixelBuffer};

use super::{FEED, HERE, PAGE, pick, play_picked, strings};
use crate::{AppWindow, SoundsData, Theme};

/// The Placed by choices, as the page names them, and what each places the stars by.
const PLACINGS: [(&str, Placing); 3] =
    [("Both", Placing::Both), ("Deep listening", Placing::Clap), ("Measurements", Placing::Own)];
/// How near the mouse (in the window's points) a star has to be to be the one under it.
const NEAR: f32 = 8.0;

/// The map as it arrives: every star by its place in the list, and each one's colour.
type Arrived = Option<(Vec<Option<StarPoint>>, Vec<i32>)>;

/// The galaxy's part of the page.
pub(super) struct Galaxy {
    /// Where the sound library is, to read its saved map; `None` (the screenshots) lays out afresh.
    pub(super) db: Option<PathBuf>,
    placing: Placing,
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
    /// Where each star was drawn, to find the one under the mouse.
    hits: Option<Arc<Hits>>,
    /// The view the picture on show was drawn for.
    drawn: Option<View>,
}

impl Default for Galaxy {
    fn default() -> Galaxy {
        Galaxy {
            db: None,
            placing: Placing::Both,
            stars: None,
            colours: None,
            loading: None,
            note: String::new(),
            view: View::default(),
            scale: 1.0,
            cancelled: false,
            asked: 0,
            hits: None,
            drawn: None,
        }
    }
}

thread_local! {
    static DRAWING: RefCell<Option<Newest<Picture>>> = const { RefCell::new(None) };
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
    let weak = app.as_weak();
    data.on_galaxy_lay_out(move || {
        if let Some(app) = weak.upgrade() {
            load(&app, true);
        }
    });
    let weak = app.as_weak();
    data.on_galaxy_reset(move || {
        let Some(app) = weak.upgrade() else { return };
        PAGE.with(|p| p.borrow_mut().galaxy.view.whole());
        draw(&app);
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
        PAGE.with(|p| p.borrow_mut().galaxy.view.zoom_at_mouse(notches));
        draw(&app);
    });
    let weak = app.as_weak();
    data.on_galaxy_down(move |x, y| {
        let Some(app) = weak.upgrade() else { return };
        PAGE.with(|p| {
            let g = &mut p.borrow_mut().galaxy;
            g.view.press(x, y);
            g.cancelled = false;
        });
        app.global::<SoundsData>().set_galaxy_grabbing(true);
    });
    let weak = app.as_weak();
    data.on_galaxy_move(move |x, y| {
        let Some(app) = weak.upgrade() else { return };
        let moved = PAGE.with(|p| p.borrow_mut().galaxy.view.moved(x, y));
        let data = app.global::<SoundsData>();
        match moved {
            Moved::Panned => {
                data.set_galaxy_hover_text("".into());
                draw(&app);
            }
            Moved::Still => {}
            Moved::Hover => {
                let found = star_at(x, y);
                let name = PAGE.with(|p| {
                    let p = p.borrow();
                    found.and_then(|at| p.list.sound(at)).map(|s| format!("{}  ·  {}", s.name(), s.sound_type))
                });
                data.set_galaxy_hover_text(name.unwrap_or_default().into());
                data.set_galaxy_mouse_x(x);
                data.set_galaxy_mouse_y(y);
            }
        }
    });
    data.on_galaxy_cancelled(|| PAGE.with(|p| p.borrow_mut().galaxy.cancelled = true));
    let weak = app.as_weak();
    data.on_galaxy_up(move || {
        let Some(app) = weak.upgrade() else { return };
        app.global::<SoundsData>().set_galaxy_grabbing(false);
        let click = PAGE.with(|p| {
            let g = &mut p.borrow_mut().galaxy;
            let cancelled = std::mem::take(&mut g.cancelled);
            g.view.release(cancelled)
        });
        let Some(at) = click.and_then(|(x, y)| star_at(x, y)) else { return };
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
    data.on_galaxy_gone(move || {
        if let Some(app) = weak.upgrade() {
            app.global::<SoundsData>().set_galaxy_hover_text("".into());
        }
    });
}

/// The shown star under (`x`, `y`) on the view, in points.
fn star_at(x: f32, y: f32) -> Option<usize> {
    PAGE.with(|p| {
        let p = p.borrow();
        let g = &p.galaxy;
        g.hits.as_ref()?.nearest(x * g.scale, y * g.scale, NEAR * g.scale)
    })
}

/// A new library: its stars are placed afresh (they're kept by place in the list).
pub(super) fn forget(g: &mut Galaxy) {
    if let Some(old) = g.loading.take() {
        old.store(true, Ordering::Relaxed);
    }
    g.stars = None;
    g.colours = None;
    g.hits = None;
    g.drawn = None;
    g.note = String::new();
}

/// The view came on show (or a new library did while it was): reads the map the first time.
pub(super) fn shown(app: &AppWindow) {
    let start = PAGE.with(|p| {
        let p = p.borrow();
        p.galaxy.stars.is_none() && p.galaxy.loading.is_none()
    });
    if start {
        // The old library's picture goes; the new map is on its way.
        let data = app.global::<SoundsData>();
        data.set_galaxy_image(Image::default());
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
        match (&stars, job) {
            (Some((_, true)), Some(job)) => job.done("Laid out afresh (kept in memory until sounds.db is trusted)"),
            (Some(_), Some(job)) => job.done("Up to date"),
            (None, Some(job)) => job.done("Stopped"),
            _ => {}
        }
        stars.map(|s| (s.0, colours))
    };
    let arrive = move |app: &AppWindow, stars: Arrived, cancel: &Arc<AtomicBool>| {
        let current = PAGE.with(|p| {
            let mut p = p.borrow_mut();
            let g = &mut p.galaxy;
            if !g.loading.as_ref().is_some_and(|c| Arc::ptr_eq(c, cancel)) {
                return false;
            }
            g.loading = None;
            if let Some((stars, colours)) = stars {
                let n = stars.iter().filter(|s| s.is_some()).count();
                g.note = if n == 0 {
                    "No sounds on the map yet: the analysis hasn't heard any.".to_owned()
                } else {
                    String::new()
                };
                g.stars = Some(Arc::new(stars));
                g.colours = Some(Arc::new(colours));
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

fn show_note(app: &AppWindow) {
    let data = app.global::<SoundsData>();
    PAGE.with(|p| {
        let g = &p.borrow().galaxy;
        data.set_galaxy_note(g.note.clone().into());
        data.set_galaxy_busy(g.loading.is_some());
    });
}

/// Draws the stars again (the map, the view, the filters or the window's size changed), off the window's thread.
pub(super) fn draw(app: &AppWindow) {
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
    let ask = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let shown = p.shown.clone();
        let g = &mut p.galaxy;
        let stars = g.stars.clone()?;
        let colours = g.colours.clone()?;
        if g.view.width < 1.0 || g.view.height < 1.0 {
            return None;
        }
        g.asked += 1;
        Some(Picture { id: g.asked, stars, colours, shown, palette, view: g.view, scale: g.scale })
    });
    let Some(ask) = ask else {
        show_picked(app);
        return;
    };
    place_picture(app);
    if HERE.with(|h| *h.borrow()) {
        let (d, px) = to_image(ask.draw());
        drawn(app, d.id, d.view, px, d.hits);
        return;
    }
    DRAWING.with(|w| {
        let mut w = w.borrow_mut();
        if w.is_none() {
            let weak = app.as_weak();
            match Newest::start("Galaxy picture", move |ask: Picture| {
                let (d, px) = to_image(background::on_pool(|| ask.draw()));
                let (id, view, hits) = (d.id, d.view, d.hits);
                let weak = weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(app) = weak.upgrade() {
                        drawn(&app, id, view, px, hits);
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

/// The picture as the window shows it (copied, off the window's thread), and what else came with it.
fn to_image(mut d: Drawn) -> (Drawn, SharedPixelBuffer<Rgba8Pixel>) {
    let mut px = SharedPixelBuffer::<Rgba8Pixel>::new(d.width as u32, d.height as u32);
    for (to, from) in px.make_mut_slice().iter_mut().zip(&d.pixels) {
        *to = Rgba8Pixel { r: from.r, g: from.g, b: from.b, a: from.a };
    }
    d.pixels = Vec::new();
    (d, px)
}

fn drawn(app: &AppWindow, id: u64, view: View, px: SharedPixelBuffer<Rgba8Pixel>, hits: Hits) {
    let current = PAGE.with(|p| {
        let g = &mut p.borrow_mut().galaxy;
        if g.asked != id {
            return false;
        }
        g.hits = Some(Arc::new(hits));
        g.drawn = Some(view);
        true
    });
    if current {
        app.global::<SoundsData>().set_galaxy_image(Image::from_rgba8_premultiplied(px));
        place_picture(app);
        show_picked(app);
    }
}

/// Moves and stretches the last picture to where its stars are on the view now, at once, so the wheel and a drag
/// answer straight away; the sharp picture for the new view replaces it when it's drawn.
fn place_picture(app: &AppWindow) {
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
