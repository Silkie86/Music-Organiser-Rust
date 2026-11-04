//! What a drag out of the app picks up from the arrangement under the mouse (a clip, or a whole track by its name),
//! where it's drawn (for the dimming while it's carried), and what it carries, made on another thread (`drag_out`).
//!
//! The left button does both jobs: pressed on a clip or a track's name, it drags that out once the mouse has moved a
//! few points; pressed anywhere else, it moves the view, as the right and middle buttons do anywhere.

use mo_formats::arrangement::{LaneKind, PreviewClip, PreviewLane};

use super::{View, lane_height};
use crate::drag_out::{self, Outgoing, Pick};

/// How far, in points, the mouse moves with the left button down on a clip or a track's name before that's dragged
/// out of the app, across or up and down (as far as a line on the Sounds page, `ListLine` in `studio.slint`).
pub const DRAG_START: f64 = 4.0;

/// Said for a drag with audio in it while the song's audio files are still being looked for.
pub const STILL_LOOKING: &str = "This song's audio files are still being looked for. Try again in a moment.";

/// A clip with something a DAW can take: notes (muted or not) or audio.
fn has_something(clip: &PreviewClip) -> bool {
    !clip.notes.is_empty() || !clip.audio.is_empty()
}

impl View {
    /// Each lane with where its top is on the view and how high it's drawn, top to bottom.
    fn lane_rows(&self) -> impl Iterator<Item = (usize, &PreviewLane, f64, f64)> {
        let scale = self.lane_scale();
        let mut y = self.ruler_height() - self.top;
        self.arrangement.lanes.iter().enumerate().map(move |(i, lane)| {
            let lh = lane_height(lane) * scale;
            let top = y;
            y += lh;
            (i, lane, top, lh)
        })
    }

    /// What a drag that starts at `x`, `y` would carry out of the app: the clip there, or the whole track when it's on
    /// a track's name. Nothing on the ruler, on empty space, on an automation clip or lane, on a group's name or on
    /// the name of a track with nothing in it: there's nothing a DAW could take from those (a drag there moves the
    /// view instead).
    pub fn pick_at(&self, x: f64, y: f64) -> Option<Pick> {
        let (hx, ruler) = (self.header_width(), self.ruler_height());
        if y < ruler || y >= self.height || x < 0.0 || x >= self.width || self.width < 60.0 {
            return None;
        }
        let (i, lane, top, lh) = self.lane_rows().find(|&(_, _, top, lh)| y >= top && y < top + lh)?;
        if matches!(lane.kind, LaneKind::Automation | LaneKind::Group) {
            return None;
        }
        if x < hx {
            // A track whose clips are all muted is still picked: the drag then says so.
            return lane.clips.iter().any(has_something).then_some(Pick::Lane(i));
        }
        // The clip drawn last is on top.
        let clip = lane.clips.iter().rposition(|c| self.clip_box(c, top, lh).contains(x, y))?;
        has_something(lane.clips.get(clip)?).then_some(Pick::Clip { lane: i, clip })
    }

    /// The left button went down at `x`, `y`: on something [`View::pick_at`] picks, it's held, to be dragged out of
    /// the app once the mouse moves far enough ([`View::moved_far`]); anywhere else the view starts moving, as with
    /// [`View::press`].
    pub fn left_press(&mut self, x: f64, y: f64) {
        self.held = self.pick_at(x, y).map(|pick| (pick, x, y));
        if self.held.is_none() {
            self.press(x, y);
        }
    }

    /// With the mouse at `x`, `y`: what's held, once the mouse is [`DRAG_START`] points or more from where it was
    /// pressed, across or up and down. It's let go of then, so it's given once: the caller drags it out.
    pub fn moved_far(&mut self, x: f64, y: f64) -> Option<Pick> {
        let (pick, from_x, from_y) = self.held?;
        if (x - from_x).abs() <= DRAG_START && (y - from_y).abs() <= DRAG_START {
            return None;
        }
        self.held = None;
        Some(pick)
    }

    /// Where `pick` is drawn, as (x, y, width, height) in the view's points, cut to the part of the view it's in (a
    /// track is its name and its row across the timeline). None when it's out of sight.
    pub fn pick_box(&self, pick: Pick) -> Option<(f64, f64, f64, f64)> {
        let (hx, ruler) = (self.header_width(), self.ruler_height());
        let lane = match pick {
            Pick::Clip { lane, .. } | Pick::Lane(lane) => lane,
        };
        let (_, l, top, lh) = self.lane_rows().nth(lane)?;
        let (x0, y0, w, h) = match pick {
            Pick::Lane(_) => (0.0, top, self.width, lh),
            Pick::Clip { clip, .. } => {
                let b = self.clip_box(l.clips.get(clip)?, top, lh);
                (b.x.max(hx), b.y, b.x + b.w - b.x.max(hx), b.h)
            }
        };
        // Cut to the view, below the ruler.
        let (top, bottom) = (y0.max(ruler), (y0 + h).min(self.height));
        let (left, right) = (x0.max(0.0), (x0 + w).min(self.width));
        (bottom > top && right > left).then_some((left, top, right - left, bottom - top))
    }

    /// What a drag of `pick` carries out of the app, made later on another thread (it looks at the disk): the returned
    /// maker holds only what it shares with this view (the song, the samples found, the waveforms read), so the view
    /// can go on meanwhile.
    pub fn drag_out(&self, pick: Pick) -> impl FnOnce() -> Result<Outgoing, String> + Send + use<> {
        let (arrangement, found, peaks) = (self.arrangement.clone(), self.found.clone(), self.peaks.clone());
        // Until the song's audio files have been looked for, none is found yet: audio would all count as missing.
        let audio = |c: &PreviewClip| !c.audio.is_empty();
        let lane = |i: usize| self.arrangement.lanes.get(i);
        let has_audio = match pick {
            Pick::Clip { lane: i, clip } => lane(i).and_then(|l| l.clips.get(clip)).is_some_and(audio),
            Pick::Lane(i) => lane(i).is_some_and(|l| l.clips.iter().any(audio)),
        };
        let too_soon = has_audio && !self.looked;
        move || {
            if too_soon {
                return Err(STILL_LOOKING.to_owned());
            }
            let waves = |saved: &str| peaks.get(saved).map(|p| p.values.clone());
            drag_out::from_arrangement(&arrangement, pick, &found, &waves)
        }
    }
}
