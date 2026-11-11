//! A right click stops what plays, on every page (Solomon, 11 Nov: "right click to stop could also work in the main
//! app too", as it does in the test plugin): the right button let go where it went down while a sound, the MIDI page's
//! loop, a part or an audition in a DAW plays. It stops what Esc's first step stops ([`crate::stop_what_plays`]). Once
//! the mouse has moved off while the button is down, it's a drag, which still moves the Galaxy's map and the
//! arrangement.
//!
//! It's heard from the window's own events ([`crate::watchdog::watch_window`]), before Slint has them, so it works
//! wherever the mouse is: over a button, a tile or a list line too. A list line opens no menu while something plays
//! (`AppKeys.plays`), so that click only stops it; the next right click opens the menu. Every event still goes on to
//! Slint.

use mo_app::galaxy_view::STILL;
use slint::ComponentHandle;
use slint::winit_030::winit::event::{ElementState, MouseButton, WindowEvent};

use crate::AppWindow;

/// The right button, followed from the window's events.
#[derive(Debug, Default)]
pub struct RightClick {
    /// Where the mouse is in the window, in the window's pixels.
    mouse: (f64, f64),
    /// The right button went down while something played, and the mouse hasn't moved off since: letting it go stops
    /// what plays.
    armed: Option<Armed>,
}

/// Where the right button went down, and how far the mouse may go from there (across plus down, in the window's
/// pixels) before it's a drag.
#[derive(Clone, Copy, Debug)]
struct Armed {
    x: f64,
    y: f64,
    still: f64,
}

impl RightClick {
    /// The mouse moved to (`x`, `y`), in the window's pixels.
    pub fn moved(&mut self, x: f64, y: f64) {
        self.mouse = (x, y);
        if self.armed.is_some_and(|a| (x - a.x).abs() + (y - a.y).abs() > a.still) {
            self.armed = None;
        }
    }

    /// The right button went down. `plays`: whether anything plays now; `still`: how far the mouse may move before it's
    /// a drag.
    pub fn pressed(&mut self, plays: bool, still: f64) {
        let (x, y) = self.mouse;
        self.armed = plays.then_some(Armed { x, y, still });
    }

    /// The right button came up: whether that ends a right click on something playing.
    pub fn released(&mut self) -> bool {
        self.armed.take().is_some()
    }

    /// The window lost the keys (another app's window is in front): a press going on is forgotten.
    pub fn forget(&mut self) {
        self.armed = None;
    }

    /// One of the window's events, before Slint has it: a right click on something playing stops it.
    pub fn heard(&mut self, app: &slint::Weak<AppWindow>, event: &WindowEvent) {
        match event {
            WindowEvent::CursorMoved { position, .. } => self.moved(position.x, position.y),
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Right, .. } => {
                let Some(app) = app.upgrade() else { return };
                // STILL is in points, as the Galaxy counts a press that hasn't moved its map: in the window's pixels.
                let still = f64::from(STILL) * f64::from(app.window().scale_factor());
                self.pressed(crate::plays(&app), still);
            }
            WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Right, .. } => {
                if self.released()
                    && let Some(app) = app.upgrade()
                {
                    crate::stop_what_plays(&app);
                }
            }
            WindowEvent::Focused(false) => self.forget(),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pressed_at(x: f64, y: f64, plays: bool) -> RightClick {
        let mut right = RightClick::default();
        right.moved(x, y);
        right.pressed(plays, 4.0);
        right
    }

    #[test]
    fn letting_go_where_it_went_down_stops_what_plays() {
        let mut right = pressed_at(100.0, 50.0, true);
        assert!(right.released());
        // Once: the next let-go has no press of its own.
        assert!(!right.released());
    }

    #[test]
    fn a_little_wobble_is_still_a_click_but_a_drag_is_not() {
        let mut right = pressed_at(100.0, 50.0, true);
        right.moved(102.0, 48.0);
        assert!(right.released(), "2 across and 2 up is within 4");
        let mut right = pressed_at(100.0, 50.0, true);
        right.moved(103.0, 52.0);
        assert!(!right.released(), "3 across and 2 down is a drag");
        // Back where it went down after a drag is still a drag, as in the Galaxy, whose map moved meanwhile.
        let mut right = pressed_at(100.0, 50.0, true);
        right.moved(160.0, 50.0);
        right.moved(100.0, 50.0);
        assert!(!right.released());
    }

    #[test]
    fn nothing_playing_when_it_went_down_stops_nothing() {
        let mut right = pressed_at(100.0, 50.0, false);
        assert!(!right.released());
    }

    #[test]
    fn another_app_in_front_forgets_the_press() {
        let mut right = pressed_at(100.0, 50.0, true);
        right.forget();
        assert!(!right.released());
    }

    #[test]
    fn the_next_press_starts_afresh() {
        let mut right = pressed_at(100.0, 50.0, true);
        right.moved(200.0, 50.0);
        assert!(!right.released());
        right.pressed(true, 4.0);
        assert!(right.released(), "pressed again where the mouse is now, and let go there");
    }
}
