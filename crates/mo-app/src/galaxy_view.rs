//! The Sounds page's galaxy view, without a window: which part of the map is on show, how the wheel zooms round the
//! mouse, and whether a press was a click or a drag. The page draws what this says.

/// How far in the map can be zoomed.
pub const MOST_ZOOM: f32 = 60.0;
/// How far (in points) a press can move and still be a click.
pub const STILL: f32 = 4.0;
/// How much of the view's shorter side the whole map fills, so the outer stars aren't on the edge.
const FILL: f32 = 0.92;

/// The part of the map on show (the map runs from 0 to 1 both ways), and a press on it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    /// The map point in the middle of the view.
    pub cx: f32,
    pub cy: f32,
    pub zoom: f32,
    /// The view's size, in points.
    pub width: f32,
    pub height: f32,
    /// Where the mouse last was, in points.
    pub mouse: (f32, f32),
    press: Option<Press>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Press {
    x: f32,
    y: f32,
    cx: f32,
    cy: f32,
    dragged: bool,
    hold: Hold,
}

/// What a held button does as the mouse moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hold {
    /// Moves the map (the right or middle button, as in the C# app).
    Pan,
    /// Plays each star the mouse passes over (the left button); the map stays still.
    Sweep,
}

/// What a mouse move did.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Moved {
    /// Nothing pressed: show what's under the mouse.
    Hover,
    /// Pressed, but not far enough to be a drag yet: nothing changes.
    Still,
    /// The map moved: draw it again.
    Panned,
    /// Sweeping: the mouse went from here to where it is now, over the stars between.
    Swept(f32, f32),
}

impl Default for View {
    fn default() -> View {
        View { cx: 0.5, cy: 0.5, zoom: 1.0, width: 0.0, height: 0.0, mouse: (0.0, 0.0), press: None }
    }
}

impl View {
    /// Points per map unit at this zoom.
    pub fn unit(&self) -> f32 {
        self.width.min(self.height).max(1.0) * FILL * self.zoom
    }

    /// Where map point (`x`, `y`) is on the view, in points times `scale` (1 for points, the window's scale factor for
    /// pixels).
    pub fn to_view(&self, x: f32, y: f32, scale: f32) -> (f32, f32) {
        let u = self.unit();
        ((self.width / 2.0 + (x - self.cx) * u) * scale, (self.height / 2.0 + (y - self.cy) * u) * scale)
    }

    /// The map point under (`x`, `y`) on the view, in points.
    pub fn to_map(&self, x: f32, y: f32) -> (f32, f32) {
        let u = self.unit();
        (self.cx + (x - self.width / 2.0) / u, self.cy + (y - self.height / 2.0) / u)
    }

    /// The whole map.
    pub fn whole(&mut self) {
        (self.cx, self.cy, self.zoom) = (0.5, 0.5, 1.0);
    }

    /// Zooms by wheel notches (positive in) round the mouse, so the map point under it stays under it.
    pub fn zoom_at_mouse(&mut self, notches: f32) {
        let (x, y) = self.mouse;
        let (mx, my) = self.to_map(x, y);
        self.zoom = (self.zoom * 1.25f32.powf(notches)).clamp(1.0, MOST_ZOOM);
        let u = self.unit();
        (self.cx, self.cy) = (mx - (x - self.width / 2.0) / u, my - (y - self.height / 2.0) / u);
        // A drag going on carries on from here.
        if let Some(p) = &mut self.press {
            (p.x, p.y, p.cx, p.cy) = (x, y, self.cx, self.cy);
        }
    }

    pub fn press(&mut self, x: f32, y: f32, hold: Hold) {
        self.mouse = (x, y);
        self.press = Some(Press { x, y, cx: self.cx, cy: self.cy, dragged: false, hold });
    }

    /// The mouse moved to (`x`, `y`). A press only moves the map once it has gone further than [`STILL`].
    pub fn moved(&mut self, x: f32, y: f32) -> Moved {
        let last = std::mem::replace(&mut self.mouse, (x, y));
        let u = self.unit();
        let Some(p) = &mut self.press else { return Moved::Hover };
        if !p.dragged && (x - p.x).abs() + (y - p.y).abs() <= STILL {
            return Moved::Still;
        }
        // The first move past STILL sweeps from where the press was, so the star under it plays too.
        let from = if p.dragged { last } else { (p.x, p.y) };
        p.dragged = true;
        match p.hold {
            Hold::Pan => {
                (self.cx, self.cy) = (p.cx - (x - p.x) / u, p.cy - (y - p.y) / u);
                Moved::Panned
            }
            Hold::Sweep => Moved::Swept(from.0, from.1),
        }
    }

    /// The press ended: where it was, when it was a click (not a drag, and not cancelled).
    pub fn release(&mut self, cancelled: bool) -> Option<(f32, f32)> {
        let p = self.press.take()?;
        (!p.dragged && !cancelled && p.hold == Hold::Sweep).then_some((p.x, p.y))
    }

    /// What the button held down does, if one is.
    pub fn holding(&self) -> Option<Hold> {
        self.press.map(|p| p.hold)
    }

    /// The press going on sweeps and has moved: letting go stops the sound.
    pub fn sweeping(&self) -> bool {
        self.press.is_some_and(|p| p.hold == Hold::Sweep && p.dragged)
    }

    pub fn pressed(&self) -> bool {
        self.press.is_some()
    }

    /// Where a picture drawn for view `drawn` sits on this view (x, y, width, height in points), so it can be moved
    /// and stretched at once while the new picture is drawn.
    pub fn place_of(&self, drawn: &View) -> (f32, f32, f32, f32) {
        let (mx0, my0) = drawn.to_map(0.0, 0.0);
        let (mx1, my1) = drawn.to_map(drawn.width, drawn.height);
        let (x0, y0) = self.to_view(mx0, my0, 1.0);
        let (x1, y1) = self.to_view(mx1, my1, 1.0);
        (x0, y0, x1 - x0, y1 - y0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> View {
        View { width: 400.0, height: 200.0, ..View::default() }
    }

    #[test]
    fn the_whole_map_fits_the_shorter_side() {
        let v = view();
        assert_eq!(v.to_view(0.5, 0.5, 1.0), (200.0, 100.0));
        let (x, _) = v.to_view(1.0, 0.5, 2.0);
        assert!((x - (200.0 + 0.5 * 200.0 * FILL) * 2.0).abs() < 0.01);
    }

    #[test]
    fn the_wheel_keeps_the_point_under_the_mouse() {
        let mut v = view();
        v.mouse = (300.0, 50.0);
        let before = v.to_map(300.0, 50.0);
        v.zoom_at_mouse(3.0);
        let after = v.to_map(300.0, 50.0);
        assert!((before.0 - after.0).abs() < 1e-5 && (before.1 - after.1).abs() < 1e-5);
        v.zoom_at_mouse(-40.0);
        assert_eq!(v.zoom, 1.0);
    }

    #[test]
    fn a_small_wobble_is_a_click_and_moves_nothing() {
        let mut v = view();
        v.press(100.0, 100.0, Hold::Sweep);
        assert_eq!(v.moved(102.0, 101.0), Moved::Still);
        assert_eq!((v.cx, v.cy), (0.5, 0.5));
        assert_eq!(v.release(false), Some((100.0, 100.0)));
        assert_eq!(v.moved(10.0, 10.0), Moved::Hover);
    }

    #[test]
    fn a_drag_moves_the_map_and_isnt_a_click() {
        let mut v = view();
        v.press(100.0, 100.0, Hold::Pan);
        assert_eq!(v.moved(150.0, 100.0), Moved::Panned);
        assert!(v.cx < 0.5);
        assert_eq!(v.release(false), None);
    }

    #[test]
    fn a_left_drag_sweeps_from_the_press_and_leaves_the_map_still() {
        let mut v = view();
        v.press(100.0, 100.0, Hold::Sweep);
        assert_eq!(v.moved(102.0, 100.0), Moved::Still);
        assert!(!v.sweeping());
        assert_eq!(v.moved(110.0, 100.0), Moved::Swept(100.0, 100.0));
        assert_eq!(v.moved(120.0, 105.0), Moved::Swept(110.0, 100.0));
        assert!(v.sweeping());
        assert_eq!((v.cx, v.cy), (0.5, 0.5));
        assert_eq!(v.release(false), None);
    }

    #[test]
    fn a_right_click_without_a_drag_isnt_a_pick() {
        let mut v = view();
        v.press(100.0, 100.0, Hold::Pan);
        assert_eq!(v.release(false), None);
    }

    #[test]
    fn a_cancelled_press_isnt_a_click() {
        let mut v = view();
        v.press(100.0, 100.0, Hold::Sweep);
        assert_eq!(v.release(true), None);
    }

    #[test]
    fn zooming_during_a_drag_carries_on_from_where_the_map_is() {
        let mut v = view();
        v.press(100.0, 100.0, Hold::Pan);
        v.moved(150.0, 100.0);
        v.zoom_at_mouse(2.0);
        let (cx, cy) = (v.cx, v.cy);
        // A move to where the mouse already is changes nothing.
        v.moved(150.0, 100.0);
        assert!((v.cx - cx).abs() < 1e-6 && (v.cy - cy).abs() < 1e-6);
    }

    #[test]
    fn an_old_picture_moves_and_grows_with_the_view() {
        let mut v = View { width: 400.0, height: 300.0, ..View::default() };
        let drawn = v;
        assert_eq!(v.place_of(&drawn), (0.0, 0.0, 400.0, 300.0));
        // Zoomed in round the middle: the old picture grows round the middle too.
        v.mouse = (200.0, 150.0);
        v.zoom_at_mouse(1.0);
        let (x, y, w, h) = v.place_of(&drawn);
        assert!((w - 500.0).abs() < 0.01 && (h - 375.0).abs() < 0.01);
        assert!((x + 50.0).abs() < 0.01 && (y + 37.5).abs() < 0.01);
    }
}
