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

/// The liked bar's tooltip: "Favourite, in 4 projects, dragged out 6 times, played 12 times".
pub fn explain(s: &Sound, usage: Option<&Usage>) -> String {
    let a = &s.activity;
    let times = |n: i32| if n == 1 { "once".to_owned() } else { format!("{n} times") };
    let mut parts = Vec::new();
    if s.favourite {
        parts.push("Favourite".to_owned());
    }
    match usage.map_or(0, |u| u.projects) {
        0 => {}
        1 => parts.push("in 1 project".to_owned()),
        n => parts.push(format!("in {n} projects")),
    }
    if a.drags + a.older_drags > 0 {
        parts.push(format!("dragged out {}", times(a.drags + a.older_drags)));
    }
    if a.sends + a.older_sends > 0 {
        parts.push(format!("sent into a DAW {}", times(a.sends + a.older_sends)));
    }
    if a.plays + a.older_plays > 0 {
        parts.push(format!("played {}", times(a.plays + a.older_plays)));
    }
    if parts.is_empty() {
        return "Not used, played or dragged out yet".to_owned();
    }
    let text = parts.join(", ");
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |c| c.to_uppercase().chain(chars).collect())
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
        let u = Usage { projects: 3, recent: 1, years: vec![] };
        // Projects 5 * (1 + 2 * 0.5) + drags 3 * 1 + sends 3 * (2 * 0.5) + plays 1 * (2 + 4 * 0.5) + favourite 10.
        assert_eq!(LikedWeights::default().score(&s, Some(&u)), 10.0 + 3.0 + 3.0 + 4.0 + 10.0);
        assert_eq!(
            explain(&s, Some(&u)),
            "Favourite, in 3 projects, dragged out once, sent into a DAW 2 times, played 6 times"
        );
        let plain = sound(2, "y.wav");
        assert_eq!(LikedWeights::default().score(&plain, None), 0.0);
        assert_eq!(explain(&plain, None), "Not used, played or dragged out yet");
        assert_eq!(explain(&plain, Some(&Usage { projects: 1, ..Usage::default() })), "In 1 project");
    }
}
