//! What a drag out of the app picks up from the arrangement under the mouse (a clip, or a whole track by its name),
//! where it's drawn (for the dimming while it's carried), and what it carries, made on another thread (`drag_out`).

use mo_formats::arrangement::{LaneKind, PreviewLane};

use super::{View, lane_height};
use crate::drag_out::{self, Outgoing, Pick};

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
    /// a track's name. Nothing on the ruler, on empty space, on an automation clip or lane, or on a group's name:
    /// there's nothing a DAW could take from those (a drag there moves the view instead).
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
            return Some(Pick::Lane(i));
        }
        // The clip drawn last is on top.
        let clip = lane.clips.iter().rposition(|c| self.clip_box(c, top, lh).contains(x, y))?;
        let c = lane.clips.get(clip)?;
        (!c.notes.is_empty() || !c.audio.is_empty()).then_some(Pick::Clip { lane: i, clip })
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

    /// What a drag of `pick` carries out of the app, made later on another thread: the returned maker holds only what
    /// it shares with this view (the song, the samples found, the waveforms read), so the view can go on meanwhile.
    pub fn drag_out(&self, pick: Pick) -> impl FnOnce() -> Result<Outgoing, String> + Send + use<> {
        let (arrangement, found, peaks) = (self.arrangement.clone(), self.found.clone(), self.peaks.clone());
        move || {
            let waves = |saved: &str| peaks.get(saved).map(|p| p.values.clone());
            drag_out::from_arrangement(&arrangement, pick, &found, &waves)
        }
    }
}
