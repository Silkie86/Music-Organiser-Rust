//! "Most liked": how much each sign that you like a sound counts, as the C# app's `LikedWeights` (Solomon's starting
//! values; adjustable in Settings, kept in `library.db`'s settings under [`LIKED_WEIGHTS_KEY`]): each project using
//! it, each drag out of the app or send into a DAW, each deliberate play, and being a favourite. Anything older than
//! a year counts [`LikedWeights::older_share`] as much.

use serde::{Deserialize, Serialize};

use crate::Sound;
use crate::usage::Usage;

/// Where the C# app keeps the weights, in `library.db`'s settings.
pub const LIKED_WEIGHTS_KEY: &str = "liked_weights";

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct LikedWeights {
    pub project: f64,
    pub drag: f64,
    pub send: f64,
    pub play: f64,
    pub favourite: f64,
    pub older_share: f64,
}

impl Default for LikedWeights {
    fn default() -> LikedWeights {
        LikedWeights { project: 5.0, drag: 3.0, send: 3.0, play: 1.0, favourite: 10.0, older_share: 0.5 }
    }
}

impl LikedWeights {
    /// The weights as saved; the defaults when none are saved or they can't be read (a weight left out is its
    /// default, as in C#).
    pub fn from_json(json: Option<&str>) -> LikedWeights {
        json.filter(|j| !j.trim().is_empty()).and_then(|j| serde_json::from_str(j).ok()).unwrap_or_default()
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// The sound's score: higher is more liked. `usage` is its use in your projects (see [`crate::usage`]).
    pub fn score(&self, s: &Sound, usage: Option<&Usage>) -> f64 {
        let a = &s.activity;
        let both = |recent: i32, older: i32| f64::from(recent) + f64::from(older) * self.older_share;
        let (all, recent) = usage.map_or((0, 0), |u| (u.projects as i32, u.recent as i32));
        self.project * both(recent, (all - recent).max(0))
            + self.drag * both(a.drags, a.older_drags)
            + self.send * both(a.sends, a.older_sends)
            + self.play * both(a.plays, a.older_plays)
            + if s.favourite { self.favourite } else { 0.0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SoundActivity;
    use crate::test_support::sound;

    #[test]
    fn weights_read_as_the_c_sharp_app_saves_them() {
        assert_eq!(LikedWeights::from_json(None), LikedWeights::default());
        assert_eq!(LikedWeights::from_json(Some("  ")), LikedWeights::default());
        assert_eq!(LikedWeights::from_json(Some("not json")), LikedWeights::default());
        let w = LikedWeights::from_json(Some(r#"{"Project":8,"Favourite":2}"#));
        assert_eq!((w.project, w.favourite, w.drag), (8.0, 2.0, 3.0));
        assert_eq!(LikedWeights::from_json(Some(&w.to_json())), w);
        assert!(w.to_json().contains("\"OlderShare\":0.5"));
    }

    #[test]
    fn the_score_counts_older_signs_less() {
        let mut s = sound(1, "x.wav");
        s.favourite = true;
        s.activity = SoundActivity { plays: 2, older_plays: 4, drags: 1, older_drags: 0, sends: 0, older_sends: 2 };
        let u = Usage { projects: 3, recent: 1, ..Usage::default() };
        // Projects 5 * (1 + 2 * 0.5) + drags 3 * 1 + sends 3 * (2 * 0.5) + plays 1 * (2 + 4 * 0.5) + favourite 10.
        assert_eq!(LikedWeights::default().score(&s, Some(&u)), 10.0 + 3.0 + 3.0 + 4.0 + 10.0);
        let plain = sound(2, "y.wav");
        assert_eq!(LikedWeights::default().score(&plain, None), 0.0);
    }
}
