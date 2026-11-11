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
use mo_app::galaxy_picture::Picture;
use mo_app::galaxy_view::{View, most_zoom_for};
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

/// Each sound's star, by the sound's place in the list; `None` for a sound the map hasn't placed.
pub type Stars = Arc<Vec<Option<StarPoint>>>;

/// Where each sound of the list sits, and its colour.
#[derive(Clone)]
pub struct Map {
    pub stars: Stars,
    /// Each sound's type's colour, as its place in the Sounds palette (-1 for grey).
    pub colours: Arc<Vec<i32>>,
    /// How many are placed.
    pub count: usize,
}

impl Map {
    /// The map of `sounds` (the list, in its order) from where the app kept each (`places`, by sound id).
    pub fn of(sounds: &[Sound], places: &HashMap<i64, StarPoint>) -> Map {
        let stars: Vec<Option<StarPoint>> =
            sounds.iter().map(|s| places.get(&s.id).copied().filter(|p| p.x.is_finite() && p.y.is_finite())).collect();
        let count = stars.iter().flatten().count();
        let colours = sounds.iter().map(|s| mo_app::sounds::type_colour(s.sound_type.as_str())).collect();
        Map { stars: Arc::new(stars), colours: Arc::new(colours), count }
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
    /// The showing the stars were last asked to be packed for the card: asked once.
    packing: Option<u64>,
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
        self.changed_stars();
    }

    /// The filters let these through now (places in the list).
    pub fn set_shown(&mut self, shown: Arc<Vec<usize>>) {
        self.shown = shown;
        self.changed_stars();
    }

    fn changed_stars(&mut self) {
        self.set += 1;
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
}
