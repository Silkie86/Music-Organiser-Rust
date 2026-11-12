//! The Galaxy in the plugin: every sound a star, sounds alike near each other, as on the app's Sounds page.
//!
//! The plugin lays nothing out itself: it reads the map the app last kept in its sound library, and draws it with the
//! app's own picture maker (`mo_app::galaxy_picture`, on every core, off the window's thread), its own view
//! (`mo_app::galaxy_view`: the wheel flies in and out, the right button moves the map, the left plays the stars it
//! passes) and its own grid for finding the star under the mouse (`mo_app::galaxy_grid`). The list's filters and
//! search box narrow the stars as they narrow the list.

use crate::library::Library;
use mo_app::galaxy_grid::{Crowd, Grid};
use mo_app::galaxy_look::{self, Look, on_view};
use mo_app::galaxy_names::{self, Label, NO_KIND, Region};
use mo_app::galaxy_picture::Picture;
use mo_app::galaxy_view::{View, most_zoom_for};
use mo_app::songs::thousands;
use mo_sounds::{Sound, StarPoint};
use std::collections::HashMap;
use std::sync::Arc;

/// The maps the app keeps, by what the stars were placed by: the one the app shows first, then the others.
pub const SPACES: [&str; 3] = ["clap+features", "clap", "features"];

/// How near the mouse (in the window's points) a star has to be to be the one under it (as in the app).
pub const NEAR: f32 = 10.0;

/// The colour of a star whose sound has no type yet (as in the app).
pub const GREY: [u8; 3] = [110, 110, 110];

/// A sweep looks for a star every so many points of the mouse's way, and no more often than this in one move.
const SWEEP_STEP: f32 = 4.0;
const MOST_SWEEP_STEPS: usize = 200;

/// While the map is moving its picture is drawn no finer than this (pixels a point), so each one comes sooner.
const MOVING_SCALE: f32 = 1.0;

/// The mini-map is this many points each way (as the app's), and comes once the view has flown in this far and is
/// this big.
pub const MINI: f32 = 132.0;
const MINI_FROM_ZOOM: f32 = 1.25;
const MINI_ROOM: (f32, f32) = (420.0, 320.0);

/// The mini-map's view: the whole map, at its own size.
fn mini_view() -> View {
    let mut view = View::default();
    (view.width, view.height) = (MINI, MINI);
    view
}

/// What the mini-map says: its line ("×4  ·  1,234 in view") and the box round the part of the map on show, as parts
/// of the mini-map (0 to 1 each way).
#[derive(Debug, PartialEq)]
pub struct Mini {
    pub title: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// The most sounds the sweep's trail and the Swept card hold (as the app's).
pub const TRAIL: usize = 16;

/// Each sound's star, by the sound's place in the list; `None` for a sound the map hasn't placed.
pub type Stars = Arc<Vec<Option<StarPoint>>>;

/// Each sound's type, for the names over the map: a place in `names` by the sound's place in the list ([`NO_KIND`]
/// for none), and each type's colour (as the app's `kinds_of`).
#[derive(Default)]
pub struct Kinds {
    of: Vec<u32>,
    names: Vec<String>,
    colours: Vec<i32>,
}

impl Kinds {
    /// A sound with no type, or "Unknown", names nothing.
    fn of(sounds: &[Sound]) -> Kinds {
        let mut kinds = Kinds::default();
        let mut ids: HashMap<&str, u32> = HashMap::new();
        for s in sounds {
            let t = s.sound_type.as_str();
            if t.is_empty() || t == mo_sounds::types::UNKNOWN {
                kinds.of.push(NO_KIND);
                continue;
            }
            let id = *ids.entry(t).or_insert_with(|| {
                kinds.names.push(t.to_owned());
                kinds.colours.push(mo_app::sounds::type_colour(t));
                (kinds.names.len() - 1) as u32
            });
            kinds.of.push(id);
        }
        kinds
    }
}

/// What the names over the map are found from ([`Naming::find`]), with the number to hand them back with
/// ([`Sky::set_regions`]).
pub struct Naming {
    pub set: u64,
    stars: Stars,
    kinds: Arc<Kinds>,
    shown: Arc<Vec<usize>>,
}

impl Naming {
    /// The neighbourhoods of the map where one sound type is most of the shown stars, biggest first. Slow with many
    /// stars (call it off the panel's thread).
    pub fn find(&self) -> Vec<Region> {
        galaxy_names::find(&self.stars, &self.kinds.of, &self.kinds.names, &self.kinds.colours, &self.shown)
    }
}

/// Where each sound of the list sits, and its colour.
#[derive(Clone)]
pub struct Map {
    pub stars: Stars,
    /// Each sound's type's colour, as its place in the Sounds palette (-1 for grey).
    pub colours: Arc<Vec<i32>>,
    /// How many are placed.
    pub count: usize,
    /// Each sound's type, for the names over the map.
    pub kinds: Arc<Kinds>,
}

impl Map {
    /// The map of `sounds` (the list, in its order) from where the app kept each (`places`, by sound id).
    pub fn of(sounds: &[Sound], places: &HashMap<i64, StarPoint>) -> Map {
        let stars: Vec<Option<StarPoint>> =
            sounds.iter().map(|s| places.get(&s.id).copied().filter(|p| p.x.is_finite() && p.y.is_finite())).collect();
        let count = stars.iter().flatten().count();
        let colours = sounds.iter().map(|s| mo_app::sounds::type_colour(s.sound_type.as_str())).collect();
        Map { stars: Arc::new(stars), colours: Arc::new(colours), count, kinds: Arc::new(Kinds::of(sounds)) }
    }
}

/// The map of the library's sounds as the app last kept it. Slow (call it off the panel's thread). An error is what to
/// tell the user.
pub fn read(library: &Library) -> Result<Map, String> {
    read_from(&mo_app::paths::data_dir().join("sounds.db"), library)
}

/// [`read`] from the app's own sound library beside `sounds_db`: opened, read and let go again.
pub fn read_from(sounds_db: &std::path::Path, library: &Library) -> Result<Map, String> {
    const NONE: &str = "No galaxy yet: open the Galaxy on the app's Sounds page once to lay it out";
    let started = std::time::Instant::now();
    let copy = mo_sounds::compact::copy_of(sounds_db);
    if !copy.is_file() {
        return Err(NONE.into());
    }
    let db = mo_sounds::SoundsDb::open_read_only(&copy).map_err(|e| format!("Couldn't open the sound library: {e}"))?;
    for space in SPACES {
        let places = db.map(space).map_err(|e| format!("Couldn't read the galaxy: {e}"))?;
        let map = Map::of(library.list.sounds(), &places);
        if map.count > 0 {
            crate::log::took("Galaxy", &format!("Read the map \"{space}\": {} stars", map.count), started);
            return Ok(map);
        }
    }
    Err(NONE.into())
}

/// The stars to be made ready for the graphics card (`mo_sky`), with the number to hand them back with
/// ([`Sky::packed`]).
pub struct Pack {
    pub set: u64,
    stars: Stars,
    colours: Arc<Vec<i32>>,
    shown: Arc<Vec<usize>>,
    palette: Vec<[u8; 3]>,
}

impl Pack {
    /// The stars as the card takes them, and how many at their end are lit (none: the plugin has no Light by yet).
    /// On every core (call it off the panel's thread).
    pub fn pack(&self) -> (Vec<u8>, u32) {
        galaxy_look::pack(&self.stars, &self.colours, &self.palette, &self.shown, &[], &[])
    }
}

/// The Galaxy's state on the panel's thread: the map, which stars are shown, the view, and which picture is on show.
#[derive(Default)]
pub struct Sky {
    map: Option<Map>,
    /// The stars the filters let through: places in the list.
    shown: Arc<Vec<usize>>,
    grid: Option<Arc<Grid>>,
    /// Counts the maps and showings: a grid made for an older one is dropped.
    set: u64,
    /// The showing a grid was last asked for: it's asked for once.
    making: Option<u64>,
    pub view: View,
    /// Pixels a point: the screen's scaling.
    scale: f32,
    /// Every sound type's colour, then grey.
    pub palette: Vec<[u8; 3]>,
    /// Counts the pictures asked for.
    asked: u64,
    /// The showing the names over the map were last asked for: asked once. And the neighbourhoods found.
    naming: Option<u64>,
    regions: Vec<Region>,
    /// The star under the mouse and the star last played, as places in the list: each gets a ring.
    pub hover: Option<usize>,
    pub picked: Option<usize>,
    /// The showing the stars were last asked to be packed for the card: asked once.
    packing: Option<u64>,
    /// The showing the mini-map's picture was last asked for: asked once.
    mini: Option<u64>,
    /// The sounds swept, clicked and gone to with the arrows lately, newest first, by place in the list; and the one
    /// the Swept card's Back and Next are on.
    trail: Vec<usize>,
    trail_at: usize,
    /// A picture is being drawn now.
    drawing: bool,
    /// The view or the stars changed since the last picture was asked for.
    stale: bool,
    /// The picture on show: its number and the view it was drawn for.
    drawn: Option<(u64, View)>,
}

impl Sky {
    pub fn has_map(&self) -> bool {
        self.map.is_some()
    }

    /// A new map: every star is back in view.
    pub fn set_map(&mut self, map: Map) {
        self.view.most = most_zoom_for(map.count);
        self.map = Some(map);
        // Another library's sounds: the trail's places in the list aren't theirs.
        self.clear_trail();
        self.changed_stars();
    }

    /// The filters let these through now (places in the list).
    pub fn set_shown(&mut self, shown: Arc<Vec<usize>>) {
        self.shown = shown;
        // The rings go: their stars may be ones the filters leave out now.
        (self.hover, self.picked) = (None, None);
        self.changed_stars();
    }

    fn changed_stars(&mut self) {
        self.set += 1;
        self.regions = Vec::new();
        self.grid = None;
        self.stale = true;
    }

    /// What a grid is made from now, with the number to hand it back with ([`Sky::set_grid`]): `None` with no map,
    /// or when it's made or being made already. Making one is slow with many stars (call [`Grid::new`] off the
    /// panel's thread).
    pub fn grid_ask(&mut self) -> Option<(u64, Stars, Arc<Vec<usize>>)> {
        let map = self.map.as_ref()?;
        if self.grid.is_some() || self.making == Some(self.set) {
            return None;
        }
        self.making = Some(self.set);
        Some((self.set, Arc::clone(&map.stars), Arc::clone(&self.shown)))
    }

    /// Takes a grid made for asking `set`, unless the stars have changed since.
    pub fn set_grid(&mut self, set: u64, grid: Grid) {
        if set == self.set {
            self.grid = Some(Arc::new(grid));
            self.stale = true;
        }
    }

    /// The view is `width` by `height` points now.
    pub fn resize(&mut self, width: f32, height: f32) {
        if (self.view.width, self.view.height) != (width, height) {
            (self.view.width, self.view.height) = (width, height);
            self.stale = true;
        }
    }

    /// The screen's scaling is `scale` pixels a point now.
    pub fn set_scale(&mut self, scale: f32) {
        if self.scale != scale {
            self.scale = scale;
            self.stale = true;
        }
    }

    /// The view moved (a glide, a pan): a new picture is wanted.
    pub fn moved(&mut self) {
        self.stale = true;
    }

    /// What the names over the map are found from now ([`Naming::find`] off the panel's thread, then
    /// [`Sky::set_regions`]): `None` with no map, or when this showing's were asked for already.
    pub fn names_ask(&mut self) -> Option<Naming> {
        let map = self.map.as_ref()?;
        if self.naming == Some(self.set) {
            return None;
        }
        self.naming = Some(self.set);
        Some(Naming {
            set: self.set,
            stars: Arc::clone(&map.stars),
            kinds: Arc::clone(&map.kinds),
            shown: Arc::clone(&self.shown),
        })
    }

    /// Takes the neighbourhoods found for asking `set`, unless the stars have changed since.
    pub fn set_regions(&mut self, set: u64, regions: Vec<Region>) {
        if set == self.set {
            self.regions = regions;
        }
    }

    /// The names where they are on the view now: the biggest few, fading as the view flies in, none over another.
    pub fn names(&self) -> Vec<Label> {
        // About a capital's width with its letter spacing, and the line with its padding (as the app guesses them).
        galaxy_names::place(&self.regions, &self.view, |name| (name.chars().count() as f32 * 10.5 + 16.0, 22.0))
    }

    /// Flies into the neighbourhood a clicked name is over (its place in the regions found).
    pub fn fly_into_name(&mut self, region: usize) {
        if let Some(region) = self.regions.get(region) {
            self.view.fly_into(region.area);
            self.stale = true;
        }
    }

    /// Where the star of the sound at `at` in the list is on the view, in points: `None` when it's off the view
    /// or has no star.
    pub fn where_is(&self, at: usize) -> Option<(f32, f32)> {
        let star = (*self.map.as_ref()?.stars.get(at)?)?;
        let (x, y) = self.view.to_view(star.x, star.y, 1.0);
        let on_view = (0.0..=self.view.width).contains(&x) && (0.0..=self.view.height).contains(&y);
        on_view.then_some((x, y))
    }

    /// What the card's stars are made from now ([`Pack::pack`] it off the panel's thread, then [`Sky::packed`]):
    /// `None` with no map, or when this showing's were asked for already.
    pub fn pack_ask(&mut self) -> Option<Pack> {
        let map = self.map.as_ref()?;
        if self.packing == Some(self.set) {
            return None;
        }
        self.packing = Some(self.set);
        Some(Pack {
            set: self.set,
            stars: Arc::clone(&map.stars),
            colours: Arc::clone(&map.colours),
            shown: Arc::clone(&self.shown),
            palette: self.palette.clone(),
        })
    }

    /// Whether stars packed for asking `set` are still the ones to show (the filters haven't changed since): then
    /// they go to the card, and the next picture is of them.
    pub fn packed(&mut self, set: u64) -> bool {
        let current = set == self.set;
        if current {
            self.stale = true;
        }
        current
    }

    /// The picture the card is to draw now, if one is wanted: the view, the pixels a point and the look. The card
    /// draws at once, on the panel's thread, so the picture is on show as soon as this is asked: it sits exactly on
    /// the view.
    pub fn on_card(&mut self) -> Option<(View, f32, Look)> {
        if !self.stale || self.view.width < 1.0 || self.view.height < 1.0 {
            return None;
        }
        self.map.as_ref()?;
        let (l, t, r, b) = on_view(&self.view);
        let crowd = match &self.grid {
            Some(grid) => grid.crowd(l, t, r, b),
            None => Crowd::even(&self.view, self.shown.len()),
        };
        self.asked += 1;
        self.stale = false;
        self.drawn = Some((self.asked, self.view));
        Some((self.view, self.scale.max(0.25), galaxy_look::look(&self.view, crowd, 0.0)))
    }

    /// The next picture to draw, if one is wanted and none is being drawn: [`Picture::draw`] it off the panel's
    /// thread and hand it to [`Sky::took`].
    pub fn picture(&mut self) -> Option<Picture> {
        if !self.stale || self.drawing || self.view.width < 1.0 || self.view.height < 1.0 {
            return None;
        }
        let map = self.map.as_ref()?;
        let (l, t, r, b) = on_view(&self.view);
        let crowd = match &self.grid {
            Some(grid) => grid.crowd(l, t, r, b),
            None => Crowd::even(&self.view, self.shown.len()),
        };
        let moving = self.view.flying() || self.view.panning();
        self.asked += 1;
        self.drawing = true;
        self.stale = false;
        Some(Picture {
            id: self.asked,
            stars: Arc::clone(&map.stars),
            count: map.count,
            colours: Arc::clone(&map.colours),
            shown: Arc::clone(&self.shown),
            light: Arc::default(),
            order: Arc::default(),
            palette: self.palette.clone(),
            view: self.view,
            scale: if moving { self.scale.min(MOVING_SCALE) } else { self.scale }.max(0.25),
            crowd,
            glows: true,
        })
    }

    /// Picture `id` was drawn, for `view`. Whether it's to be shown (it's newer than the one on show).
    pub fn took(&mut self, id: u64, view: View) -> bool {
        if id == self.asked {
            self.drawing = false;
        }
        if self.drawn.is_some_and(|(shown, _)| shown >= id) {
            return false;
        }
        self.drawn = Some((id, view));
        true
    }

    /// Picture `id` couldn't be drawn: the next one may be asked for.
    pub fn gave_up(&mut self, id: u64) {
        if id == self.asked {
            self.drawing = false;
        }
    }

    /// The mini-map's picture to draw now ([`Picture::draw`] it off the panel's thread, then [`Sky::mini_took`] with
    /// its `id`): the whole map as the filters leave it, in plain dots. `None` with no map, or when this showing's
    /// was asked for already.
    pub fn mini_ask(&mut self) -> Option<Picture> {
        let map = self.map.as_ref()?;
        if self.mini == Some(self.set) {
            return None;
        }
        self.mini = Some(self.set);
        let view = mini_view();
        Some(Picture {
            id: self.set,
            stars: Arc::clone(&map.stars),
            count: map.count,
            colours: Arc::clone(&map.colours),
            shown: Arc::clone(&self.shown),
            light: Arc::default(),
            order: Arc::default(),
            palette: self.palette.clone(),
            view,
            scale: self.scale.max(0.25),
            crowd: Crowd::even(&view, self.shown.len()),
            glows: false,
        })
    }

    /// Whether a mini-map picture drawn for asking `set` is still the one to show (the filters haven't changed).
    pub fn mini_took(&self, set: u64) -> bool {
        set == self.set
    }

    /// The mini-map as it is now: `None` until the view has flown in (and while there's no room for it).
    pub fn mini(&self) -> Option<Mini> {
        let view = &self.view;
        let count = self.map.as_ref()?.count;
        if view.zoom <= MINI_FROM_ZOOM || view.width < MINI_ROOM.0 || view.height < MINI_ROOM.1 || count == 0 {
            return None;
        }
        let mini = mini_view();
        let (l, t, r, b) = on_view(view);
        let corner = |x: f32, y: f32| {
            let (px, py) = mini.to_view(x, y, 1.0);
            ((px / MINI).clamp(0.0, 1.0), (py / MINI).clamp(0.0, 1.0))
        };
        let ((x0, y0), (x1, y1)) = (corner(l, t), corner(r, b));
        let title = match &self.grid {
            Some(grid) => format!("×{:.0}  ·  {} in view", view.zoom, thousands(grid.count_in(l, t, r, b))),
            None => format!("×{:.0}", view.zoom),
        };
        Some(Mini { title, x: x0, y: y0, width: x1 - x0, height: y1 - y0 })
    }

    /// A press on the mini-map, (`x`, `y`) of the way across and down it: the view glides there, as far in as it is.
    pub fn mini_go(&mut self, x: f32, y: f32) {
        let (x, y) = mini_view().to_map(x.clamp(0.0, 1.0) * MINI, y.clamp(0.0, 1.0) * MINI);
        let zoom = self.view.zoom;
        self.view.fly_to(x.clamp(0.0, 1.0), y.clamp(0.0, 1.0), zoom);
        self.stale = true;
    }

    /// An arrow key (`way`: (1, 0) for Right, (0, 1) for Down): the nearest star that way from the picked one (or
    /// from the middle of the view) is picked, and the map glides to keep it in sight, as in the app. The star to
    /// play: its place in the list. `None` when there's no star that way, or no grid yet.
    pub fn arrow(&mut self, way: (f32, f32)) -> Option<usize> {
        let stars = &self.map.as_ref()?.stars;
        let star = |at: usize| stars.get(at).copied().flatten();
        let from = self.picked.and_then(star).map_or((self.view.cx, self.view.cy), |s| (s.x, s.y));
        // A view moved off the map starts from its nearest edge, so the stars on the map are still found.
        let from = (from.0.clamp(0.0, 1.0), from.1.clamp(0.0, 1.0));
        // At least a star's width on the view, so each press shows a move.
        let least = NEAR / self.view.unit().max(f32::MIN_POSITIVE);
        let at = self.grid.as_ref()?.toward(from.0, from.1, way, least)?;
        let to = star(at)?;
        self.picked = Some(at);
        // Off the view or near its edge: the map glides (as far in as it is) to put the star in the middle.
        let (x, y) = self.view.to_view(to.x, to.y, 1.0);
        let (width, height) = (self.view.width, self.view.height);
        let edge = 0.1 * width.min(height);
        if x < edge || y < edge || x > width - edge || y > height - edge {
            let zoom = self.view.zoom;
            self.view.fly_to(to.x, to.y, zoom);
            self.stale = true;
        }
        Some(at)
    }

    /// Puts a sound on the trail, newest first (as the app: a swept one moves to the front; a clicked one already on
    /// it stays where it is), and Back and Next start from it.
    pub fn on_trail(&mut self, at: usize, swept: bool) {
        match self.trail.iter().position(|&on| on == at) {
            Some(i) if !swept => self.trail_at = i,
            found => {
                if let Some(i) = found {
                    self.trail.remove(i);
                }
                self.trail.insert(0, at);
                self.trail.truncate(TRAIL);
                self.trail_at = 0;
            }
        }
    }

    /// The trail, newest first (places in the list), and which of them Back and Next are on.
    pub fn trail(&self) -> (&[usize], usize) {
        (&self.trail, self.trail_at)
    }

    pub fn clear_trail(&mut self) {
        self.trail.clear();
        self.trail_at = 0;
    }

    /// Goes to the `i`th sound on the trail (0 the newest): it's picked and the map glides to it, as far in as it
    /// is. The sound to play: its place in the list.
    pub fn go_on_trail(&mut self, i: usize) -> Option<usize> {
        let at = *self.trail.get(i)?;
        self.trail_at = i;
        self.picked = Some(at);
        if let Some(star) = self.map.as_ref().and_then(|map| *map.stars.get(at)?) {
            let zoom = self.view.zoom;
            self.view.fly_to(star.x, star.y, zoom);
            self.stale = true;
        }
        Some(at)
    }

    /// Goes `by` sounds down the trail (1: to the one swept before) or up it (-1), stopping at its ends.
    pub fn step_on_trail(&mut self, by: i32) -> Option<usize> {
        let last = self.trail.len().checked_sub(1)?;
        let to = if by > 0 { (self.trail_at + 1).min(last) } else { self.trail_at.saturating_sub(1) };
        self.go_on_trail(to)
    }

    /// The trail on the view now: the line through its stars (as a path's commands, empty with fewer than two) and
    /// a dot for each star on or just off the view (x, y in points, and how strongly it shows: fainter with age).
    pub fn trail_on_view(&self) -> (String, Vec<(f32, f32, f32)>) {
        use std::fmt::Write;
        let Some(map) = &self.map else { return Default::default() };
        let count = self.trail.len().max(1) as f32;
        let mut on_view: Vec<(f32, f32, f32)> = Vec::new();
        for (i, &at) in self.trail.iter().enumerate() {
            if let Some(star) = map.stars.get(at).copied().flatten() {
                let (x, y) = self.view.to_view(star.x, star.y, 1.0);
                on_view.push((x, y, 1.0 - i as f32 / count));
            }
        }
        let mut path = String::new();
        if on_view.len() > 1 {
            for (i, (x, y, _)) in on_view.iter().enumerate() {
                let _ = write!(path, "{} {x:.1} {y:.1} ", if i == 0 { "M" } else { "L" });
            }
        }
        let (width, height) = (self.view.width, self.view.height);
        on_view.retain(|&(x, y, _)| x > -10.0 && y > -10.0 && x < width + 10.0 && y < height + 10.0);
        (path, on_view)
    }

    /// The shown stars the mouse passed going from `from` to `to` on the view, in the order it passed them, each once.
    pub fn passed(&self, from: (f32, f32), to: (f32, f32)) -> Vec<usize> {
        let far = (to.0 - from.0).hypot(to.1 - from.1);
        let steps = ((far / SWEEP_STEP).ceil() as usize).clamp(1, MOST_SWEEP_STEPS);
        let mut passed = Vec::new();
        for step in 0..=steps {
            let t = step as f32 / steps as f32;
            if let Some(at) = self.star_at(from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t)
                && !passed.contains(&at)
            {
                passed.push(at);
            }
        }
        passed
    }

    /// Where the picture on show sits on the view now (x, y, width, height in points): moved and stretched at once
    /// as the view moves, until the next picture comes.
    pub fn place(&self) -> Option<(f32, f32, f32, f32)> {
        self.drawn.map(|(_, drawn)| self.view.place_of(&drawn))
    }

    /// The shown star under (`x`, `y`) on the view, in points: its place in the list.
    pub fn star_at(&self, x: f32, y: f32) -> Option<usize> {
        let (mx, my) = self.view.to_map(x, y);
        self.grid.as_ref()?.nearest(mx, my, NEAR / self.view.unit().max(f32::MIN_POSITIVE))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sounds() -> Vec<Sound> {
        let mut kick = Sound::new(7, r"D:\Drums\Kick.wav");
        kick.sound_type = mo_sounds::Word::new("Kick");
        vec![kick, Sound::new(8, r"D:\Drums\Other.wav"), Sound::new(9, r"D:\Drums\Unplaced.wav")]
    }

    fn sky() -> Sky {
        let places = HashMap::from([
            (7, StarPoint { x: 0.25, y: 0.5 }),
            (8, StarPoint { x: 0.75, y: 0.5 }),
            (99, StarPoint { x: 0.1, y: 0.1 }),
        ]);
        let mut sky = Sky { palette: vec![[255, 0, 0], GREY], ..Sky::default() };
        sky.set_scale(1.0);
        sky.set_map(Map::of(&sounds(), &places));
        sky.set_shown(Arc::new(vec![0, 1, 2]));
        sky.resize(200.0, 100.0);
        sky
    }

    #[test]
    fn a_map_places_the_list_s_sounds_by_their_ids() {
        let places = HashMap::from([(7, StarPoint { x: 0.25, y: 0.5 }), (9, StarPoint { x: f32::NAN, y: 0.5 })]);
        let map = Map::of(&sounds(), &places);
        assert_eq!(*map.stars, [Some(StarPoint { x: 0.25, y: 0.5 }), None, None]);
        assert_eq!(map.count, 1);
        assert_eq!(map.colours.len(), 3);
        assert!(map.colours[0] >= 0 && map.colours[1] == -1);
    }

    #[test]
    fn no_sound_library_yet_says_how_to_get_a_galaxy() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("no-such-folder");
        let said = read_from(&dir.join("sounds.db"), &Library::new(sounds())).err().expect("no map");
        assert!(said.starts_with("No galaxy yet"), "{said}");
    }

    #[test]
    fn one_picture_at_a_time_and_only_a_newer_one_is_shown() {
        let mut sky = sky();
        let first = sky.picture().expect("a picture to draw");
        // Being drawn: a move waits for it.
        sky.moved();
        assert!(sky.picture().is_none());
        let drawn = first.draw();
        assert_eq!((drawn.width, drawn.height), (200, 100));
        assert!(drawn.pixels.iter().any(|p| p.a > 0), "the stars are drawn");
        assert!(sky.took(drawn.id, drawn.view));
        assert!(!sky.took(drawn.id, drawn.view), "the same picture isn't shown twice");
        // The move's picture is asked for now; with nothing changed after it, none.
        let second = sky.picture().expect("the move's picture");
        let id = second.id;
        // It couldn't be drawn: the next one is asked for once something changes.
        sky.gave_up(id);
        assert!(sky.picture().is_none());
        sky.moved();
        let third = sky.picture().expect("the next picture").draw();
        assert!(sky.took(third.id, third.view));
        assert!(sky.picture().is_none());
        // On show where it was drawn: the whole view.
        let (x, y, w, h) = sky.place().expect("a picture on show");
        assert!(x.abs() < 0.01 && y.abs() < 0.01 && (w - 200.0).abs() < 0.01 && (h - 100.0).abs() < 0.01);
    }

    #[test]
    fn the_card_gets_the_stars_once_a_showing_and_a_picture_only_when_something_changed() {
        let mut sky = sky();
        let pack = sky.pack_ask().expect("the stars to pack");
        assert!(sky.pack_ask().is_none(), "asked once for a showing");
        // Every placed star, 16 bytes each (the unplaced third sound has none); none lit.
        let (bytes, lit) = pack.pack();
        assert_eq!((bytes.len(), lit), (2 * galaxy_look::STAR_BYTES, 0));
        assert!(sky.packed(pack.set));
        let (view, scale, _) = sky.on_card().expect("a picture for the card");
        assert_eq!((view.width, view.height, scale), (200.0, 100.0, 1.0));
        assert!(sky.on_card().is_none(), "nothing changed since");
        // On show at once, exactly on the view.
        let (x, y, w, h) = sky.place().expect("a picture on show");
        assert!(x.abs() < 0.01 && y.abs() < 0.01 && (w - 200.0).abs() < 0.01 && (h - 100.0).abs() < 0.01);
        sky.moved();
        assert!(sky.on_card().is_some());
        // The filters changed while the stars were being packed: those are dropped and asked for again.
        let old = sky.pack_ask();
        assert!(old.is_none());
        sky.set_shown(Arc::new(vec![0]));
        assert!(!sky.packed(pack.set));
        assert!(sky.pack_ask().is_some());
    }

    #[test]
    fn names_are_found_once_a_showing_and_the_rings_follow_their_stars() {
        let mut sky = sky();
        let naming = sky.names_ask().expect("names to find");
        assert!(sky.names_ask().is_none(), "asked once for a showing");
        sky.set_regions(naming.set, naming.find());
        // Three stars are too few for a neighbourhood: no names, and placing none is fine.
        assert!(sky.names().len() <= 1);
        // The first sound's star is on the view, the unplaced third has none.
        let (x, y) = sky.where_is(0).expect("on the view");
        assert!((0.0..=200.0).contains(&x) && (0.0..=100.0).contains(&y));
        assert_eq!(sky.where_is(2), None);
        // New filters: the rings go, and the names are asked for again.
        (sky.hover, sky.picked) = (Some(0), Some(1));
        sky.set_shown(Arc::new(vec![1]));
        assert_eq!((sky.hover, sky.picked), (None, None));
        sky.set_regions(naming.set, naming.find());
        assert!(sky.names().is_empty(), "names found for the old showing are dropped");
        assert!(sky.names_ask().is_some());
    }

    #[test]
    fn the_star_under_the_mouse_is_found_once_the_grid_is_made() {
        let mut sky = sky();
        let (x, y) = sky.view.to_view(0.25, 0.5, 1.0);
        assert_eq!(sky.star_at(x, y), None, "no grid yet");
        let (set, stars, shown) = sky.grid_ask().expect("a grid to make");
        assert!(sky.grid_ask().is_none(), "asked for once");
        // The stars changed while it was made: it's dropped, and asked for again.
        sky.set_shown(Arc::new(vec![0]));
        sky.set_grid(set, Grid::new(&stars, &shown));
        assert_eq!(sky.star_at(x, y), None);
        let (set, stars, shown) = sky.grid_ask().expect("a grid to make");
        sky.set_grid(set, Grid::new(&stars, &shown));
        assert!(sky.grid_ask().is_none());
        assert_eq!(sky.star_at(x, y), Some(0));
        assert_eq!(sky.star_at(x + 3.0, y), Some(0));
        // The other star is filtered out, and nothing is far from every star.
        let (ox, oy) = sky.view.to_view(0.75, 0.5, 1.0);
        assert_eq!(sky.star_at(ox, oy), None);
        assert_eq!(sky.star_at(x + 40.0, y), None);
        // A sweep across it passes it once; one that stays away passes none.
        assert_eq!(sky.passed((x - 30.0, y), (x + 30.0, y)), [0]);
        assert!(sky.passed((x - 30.0, y + 40.0), (x + 30.0, y + 40.0)).is_empty());
    }

    fn with_grid(mut sky: Sky) -> Sky {
        let (set, stars, shown) = sky.grid_ask().expect("a grid to make");
        sky.set_grid(set, Grid::new(&stars, &shown));
        sky
    }

    #[test]
    fn the_mini_map_comes_once_flown_in_and_a_press_on_it_glides_there() {
        let mut sky = with_grid(sky());
        sky.resize(600.0, 400.0);
        assert_eq!(sky.mini(), None, "the whole map is on show");
        // Its picture is asked for once a showing: the whole map, whatever the view.
        let picture = sky.mini_ask().expect("a mini-map to draw");
        assert!(sky.mini_ask().is_none());
        assert_eq!((picture.view.width, picture.view.zoom, picture.glows), (MINI, 1.0, false));
        assert!(sky.mini_took(picture.id));
        sky.view.zoom = 4.0;
        (sky.view.cx, sky.view.cy) = (0.25, 0.5);
        let mini = sky.mini().expect("a mini-map");
        assert_eq!(mini.title, "×4  ·  1 in view");
        // The box is round the part on show: about the left star, and smaller than the mini-map.
        let middle = (mini.x + mini.width / 2.0, mini.y + mini.height / 2.0);
        assert!((middle.0 - 0.25).abs() < 0.05 && (middle.1 - 0.5).abs() < 0.05, "{mini:?}");
        assert!(mini.width < 0.5 && mini.height < 0.5, "{mini:?}");
        // No room for it in a small view.
        sky.resize(300.0, 400.0);
        assert_eq!(sky.mini(), None);
        sky.resize(600.0, 400.0);
        // A press on its right half glides the view there, no further in or out.
        sky.mini_go(0.75, 0.5);
        assert!(sky.view.flying());
        sky.view.glide(std::time::Duration::from_secs(60));
        assert!((sky.view.cx - 0.75).abs() < 0.05 && (sky.view.cy - 0.5).abs() < 0.01, "{:?}", sky.view);
        assert!((sky.view.zoom - 4.0).abs() < 0.01);
        // The filters changed while a picture was drawn: it isn't shown, and a new one is asked for.
        sky.set_shown(Arc::new(vec![0]));
        assert!(!sky.mini_took(picture.id));
        assert!(sky.mini_ask().is_some());
    }

    #[test]
    fn an_arrow_key_picks_the_nearest_star_that_way_and_keeps_it_in_sight() {
        let mut sky = sky();
        assert_eq!(sky.arrow((1.0, 0.0)), None, "no grid yet");
        let mut sky = with_grid(sky);
        // From the middle of the view with no star picked.
        assert_eq!(sky.arrow((-1.0, 0.0)), Some(0));
        assert_eq!(sky.picked, Some(0));
        assert!(!sky.view.flying(), "it's well inside the view");
        assert_eq!(sky.arrow((1.0, 0.0)), Some(1), "from the picked star now");
        assert_eq!(sky.arrow((1.0, 0.0)), None, "none further right");
        assert_eq!(sky.arrow((0.0, -1.0)), None, "none above");
        assert_eq!(sky.picked, Some(1));
        // Flown in on the right star, the left one is off the view: the map glides to it.
        sky.view.zoom = 8.0;
        (sky.view.cx, sky.view.cy) = (0.75, 0.5);
        assert_eq!(sky.arrow((-1.0, 0.0)), Some(0));
        assert!(sky.view.flying());
        sky.view.glide(std::time::Duration::from_secs(60));
        assert!((sky.view.cx - 0.25).abs() < 0.01 && (sky.view.zoom - 8.0).abs() < 0.01, "{:?}", sky.view);
    }

    #[test]
    fn the_trail_keeps_the_sounds_heard_lately_newest_first() {
        let mut sky = with_grid(sky());
        assert_eq!(sky.trail_on_view(), (String::new(), Vec::new()));
        sky.on_trail(0, true);
        sky.on_trail(1, true);
        assert_eq!(sky.trail(), (&[1, 0][..], 0));
        // A clicked one already on it stays where it is; a swept one moves to the front.
        sky.on_trail(0, false);
        assert_eq!(sky.trail(), (&[1, 0][..], 1));
        sky.on_trail(0, true);
        assert_eq!(sky.trail(), (&[0, 1][..], 0));
        // The line goes through both stars, and the older one's dot is fainter.
        let (path, dots) = sky.trail_on_view();
        let (x0, y0) = sky.view.to_view(0.25, 0.5, 1.0);
        let (x1, y1) = sky.view.to_view(0.75, 0.5, 1.0);
        assert_eq!(path, format!("M {x0:.1} {y0:.1} L {x1:.1} {y1:.1} "));
        assert_eq!(dots, [(x0, y0, 1.0), (x1, y1, 0.5)]);
        // Back goes to the one before, and stops at the end; Next comes back.
        assert_eq!(sky.step_on_trail(1), Some(1));
        assert_eq!((sky.picked, sky.trail().1), (Some(1), 1));
        assert!(sky.view.flying(), "the map glides to it");
        assert_eq!(sky.step_on_trail(1), Some(1));
        assert_eq!(sky.step_on_trail(-1), Some(0));
        assert_eq!(sky.go_on_trail(5), None);
        // A sound with no star is on the card's list, not on the map's line.
        sky.on_trail(2, true);
        assert_eq!(sky.trail().0, [2, 0, 1]);
        assert_eq!(sky.trail_on_view().1.len(), 2);
        // No more than the card holds.
        for at in 10..40 {
            sky.on_trail(at, true);
        }
        assert_eq!(sky.trail().0.len(), TRAIL);
        sky.clear_trail();
        assert_eq!(sky.trail(), (&[][..], 0));
        assert_eq!(sky.step_on_trail(1), None);
    }
}
