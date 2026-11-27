//! A save's shape: how long it is, how many sections and how much automation it has, and what its mixer holds (EQ and
//! compression on its tracks, send tracks, effects on the master). The Songs board guesses a song's stage from it
//! (`mo_library::stage`): an idea, in progress, arranging, mixing or finished.
//!
//! Read from the save's bytes, once, and never written to the file. FL Studio and Bitwig only for now; a Studio One song
//! has no shape yet (`None`), so its stage is never guessed.

use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use crate::arrangement::{Arrangement, LaneKind};

/// Goes up by one whenever what a shape is read as changes (rule 7): the library reads every kept shape again.
pub const VERSION: u32 = 2;

/// What a save holds, counted.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shape {
    /// How long the song plays, in seconds: to the end of its last clip.
    pub seconds: f64,
    /// How many bars that is, in the song's own time signature.
    pub bars: f64,
    /// Markers on its timeline (FL's time markers, Bitwig's cue markers): how many sections it was marked into.
    pub markers: u32,
    /// Settings the song moves: automation clips (FL) or lanes (Bitwig) whose value changes.
    pub automation: u32,
    /// Mixer tracks with sound through them, not counting send tracks or the master.
    pub tracks: u32,
    /// Of them, those with an EQ, and those with a compressor.
    pub eq: u32,
    pub compressed: u32,
    /// Send (return, effect) tracks.
    pub sends: u32,
    /// The effects on the master, top first.
    pub master: Vec<String>,
}

impl Shape {
    /// One of the master's effects is a limiter (or a maximiser or clipper), put there to finish the song: FL
    /// Studio's Fruity Limiter isn't counted, as FL's default new project has it on the master already.
    pub fn master_limited(&self) -> bool {
        self.master_limiter().is_some()
    }

    /// The master's limiter ([`Shape::master_limited`]), when it has one.
    pub fn master_limiter(&self) -> Option<&str> {
        self.master.iter().map(String::as_str).find(|m| is_limiter(m) && !m.eq_ignore_ascii_case("Fruity Limiter"))
    }
}

/// Each mixer track's effects (not the send tracks' or the master's), the send tracks, and the master's effects.
#[derive(Debug, Default)]
pub(crate) struct Chains {
    pub tracks: Vec<Vec<String>>,
    pub sends: u32,
    pub master: Vec<String>,
}

/// The shape of a save, from its bytes, by its file's ending; `None` for a file of another kind (or a Studio One song)
/// or one too damaged to read. A Bitwig project's bytes are moved in, not copied ([`crate::bitwig::FileBytes`]).
pub fn read_bytes(path: &Path, raw: impl crate::bitwig::FileBytes + AsRef<[u8]>) -> Option<Shape> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_lowercase();
    match ext.as_str() {
        "flp" => {
            let flp = crate::fl::events::load(raw.as_ref()).ok()?;
            let song = crate::fl::song::read_song_with(&flp, false);
            let view = crate::fl::arrangement::playlist_of(&flp, &song);
            // Each setting moved once, however many clips move it.
            let mut moving: Vec<String> = song
                .automation
                .iter()
                .filter(|a| moves(a.points.iter().map(|p| p.value)))
                .map(|a| format!("{:?}/{:?}/{:?}/{:?}/{}", a.channel, a.insert, a.slot, a.control, a.parameter))
                .collect();
            moving.sort_unstable();
            moving.dedup();
            Some(shape(&view, moving.len() as u32, fl_chains(&song)))
        }
        "bwproject" => {
            let doc = crate::bitwig::decode(raw).ok()?;
            // A file whose objects didn't all decode has no arrangement to measure: no shape, rather than a wrong one.
            if doc.root.is_none() || doc.body_error.is_some() {
                return None;
            }
            let views = crate::bitwig::arrangement::read(&doc);
            let view = views.first()?;
            // Lanes on a muted or deactivated track, and muted clips, move nothing.
            let automation = view
                .lanes
                .iter()
                .filter(|l| l.kind == LaneKind::Automation && !l.muted)
                .filter(|l| moves(l.clips.iter().filter(|c| !c.muted).flat_map(|c| &c.points).map(|p| p.value)))
                .count() as u32;
            Some(shape(view, automation, crate::bitwig::reader::chains(&doc)))
        }
        _ => None,
    }
}

/// [`read_bytes`] for a file on disk, read once.
pub fn read(path: &Path) -> std::io::Result<Option<Shape>> {
    Ok(read_bytes(path, std::fs::read(path)?))
}

fn shape(view: &Arrangement, automation: u32, chains: Chains) -> Shape {
    let has = |test: fn(&str) -> bool| chains.tracks.iter().filter(|t| t.iter().any(|e| test(e))).count() as u32;
    Shape {
        seconds: view.seconds(),
        bars: view.bars(),
        markers: view.markers.len() as u32,
        automation,
        tracks: chains.tracks.len() as u32,
        eq: has(is_eq),
        compressed: has(is_compressor),
        sends: chains.sends,
        master: chains.master,
    }
}

/// The values change somewhere: a lane drawn flat moves nothing.
fn moves(mut values: impl Iterator<Item = f64>) -> bool {
    let Some(first) = values.next() else { return false };
    values.any(|v| (v - first).abs() > 1e-6)
}

/// An FL song's mixer: the inserts a channel feeds or that hold an effect, and those that are sends. An insert sending
/// its sound to several places (the master and another insert, or two inserts) is sending to those others (or
/// sidechaining them: the file doesn't tell them apart); an insert that one sends to as its only place is a bus, and
/// one a channel feeds is that channel's track (a sidechained bass), both mixer tracks like any other.
fn fl_chains(song: &crate::fl::song::FlSong) -> Chains {
    use std::collections::BTreeSet;
    // Channels that make sound: not automation clips or layers.
    let sounding = || song.channels.iter().filter(|c| c.instrument.is_some() || !c.sample.is_empty());
    let fed: BTreeSet<u32> = sounding().map(|c| c.insert).collect();
    let mut sends: BTreeSet<u32> = BTreeSet::new();
    let mut buses: BTreeSet<u32> = BTreeSet::new();
    for to in song.routes.values() {
        match to.as_slice() {
            [only] => _ = buses.insert(*only),
            many => sends.extend(many.iter().copied()),
        }
    }
    sends.remove(&0);
    let sends: BTreeSet<u32> = sends.iter().filter(|i| !buses.contains(i) && !fed.contains(i)).copied().collect();
    let effects = |i: &crate::fl::song::FlInsert| -> Vec<String> {
        i.effects.iter().filter(|e| e.enabled).map(|e| e.name.clone()).collect()
    };
    let mut chains = Chains { sends: sends.len() as u32, ..Chains::default() };
    for insert in &song.inserts {
        if insert.index == 0 {
            chains.master = effects(insert);
        } else if !sends.contains(&insert.index)
            && (fed.contains(&insert.index) || insert.effects.iter().any(|e| e.enabled))
        {
            chains.tracks.push(effects(insert));
        }
    }
    // A channel feeding the master straight is a track of its own.
    let straight = sounding().filter(|c| c.insert == 0).count();
    chains.tracks.extend(std::iter::repeat_n(Vec::new(), straight));
    chains
}

/// The name matches a fixed pattern (tested below, so it always builds; one that didn't would match nothing).
fn matches(pattern: &LazyLock<Option<Regex>>, name: &str) -> bool {
    pattern.as_ref().is_some_and(|re| re.is_match(name))
}

/// An EQ, by its name: "EQ-5", "Pro-Q 3", "Fruity parametric EQ 2", "SlickEQ", "Pultec EQP-1A", "Equalizer"...
/// Not "Frequency Shifter", Bitwig's "Freq Shifter" or a "Step Seq": "eq" counts only at the start of a word, or at
/// its end after a letter other than r or s.
pub fn is_eq(name: &str) -> bool {
    static EQ: LazyLock<Option<Regex>> = LazyLock::new(|| {
        Regex::new(r"(?i)(^|[^a-z])eq|[a-qt-z]eq([^a-z]|$)|pro-?\s?q\b|equali[sz]|pultec|tdr nova|kirchhoff").ok()
    });
    matches(&EQ, name)
}

/// A compressor, by its name: "Compressor", "Pro-C 2", "CLA-76", "1176", "LA-2A", "Glue", "OTT", FL's "Fruity
/// Limiter" (a compressor too) and "Maximus", "Dynamics"...
pub fn is_compressor(name: &str) -> bool {
    static COMP: LazyLock<Option<Regex>> = LazyLock::new(|| {
        Regex::new(
            r"(?i)(^|[^a-z])comp(ress|[^a-z]|$)|pro-?\s?c\b|cla-?(76|2a|3a)|\b1176\b|la-?2a|\bglue\b|\bott\b|fruity limiter|maximus|dynamics|vari-?mu|fairchild|bus comp|kotelnikov|pressure\b",
        )
        .ok()
    });
    matches(&COMP, name)
}

/// A limiter, maximiser or clipper, as a song's master holds once it's being finished: "Pro-L 2", "Ozone 11
/// Maximizer", "L2", "Peak Limiter", "Elevate", "Soft Clipper", "Fruity Limiter"...
pub fn is_limiter(name: &str) -> bool {
    static LIMIT: LazyLock<Option<Regex>> =
        LazyLock::new(|| Regex::new(r"(?i)limit|pro-?\s?l\b|maximi[sz]|\bl[123]\b|elevate|clipper|maximus").ok());
    matches(&LIMIT, name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_patterns_build() {
        assert!(is_eq("EQ") && is_compressor("Compressor") && is_limiter("Limiter"));
    }

    #[test]
    fn effects_are_known_by_their_names() {
        for eq in ["EQ-5", "EQ+", "Pro-Q 3", "FabFilter Pro-Q 4", "Fruity parametric EQ 2", "SlickEQ", "Pultec EQP-1A"]
        {
            assert!(is_eq(eq), "{eq}");
        }
        for not in [
            "Frequency Shifter",
            "Freq Shifter",
            "Freq Echo",
            "Step Seq",
            "Sequencer",
            "Fruity Reeverb 2",
            "Compressor",
        ] {
            assert!(!is_eq(not), "{not}");
        }
        for comp in ["Compressor", "Compressor+", "Pro-C 2", "CLA-76", "Fruity Limiter", "OTT", "Glue Compressor"] {
            assert!(is_compressor(comp), "{comp}");
        }
        for not in ["Composer", "EQ-5", "Pro-Q 3", "Complex"] {
            assert!(!is_compressor(not), "{not}");
        }
        for limit in ["Pro-L 2", "Ozone 11 Maximizer", "L2", "Peak Limiter", "Soft Clipper", "Fruity Limiter"] {
            assert!(is_limiter(limit), "{limit}");
        }
        for not in ["Pro-Q 3", "L12 Reverb", "Compressor", "Ozone Imager", "Ozone 11 Equalizer"] {
            assert!(!is_limiter(not), "{not}");
        }
        // FL's default new project has Fruity Limiter on its master: it doesn't say the song's being finished.
        let fl = Shape { master: vec!["Fruity Limiter".into()], ..Shape::default() };
        assert!(!fl.master_limited());
        let done = Shape { master: vec!["Fruity Limiter".into(), "Ozone 11 Maximizer".into()], ..Shape::default() };
        assert_eq!(done.master_limiter(), Some("Ozone 11 Maximizer"));
    }

    #[test]
    fn a_flat_lane_moves_nothing() {
        assert!(!moves([0.5, 0.5, 0.5].into_iter()));
        assert!(moves([0.5, 0.25].into_iter()));
        assert!(!moves(std::iter::empty()));
    }

    #[test]
    fn an_fl_inserts_effect_belongs_to_its_track() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/recipe/fl/MO Recipe 05 mixer.flp");
        let raw = std::fs::read(path).unwrap();
        let flp = crate::fl::events::load(&raw).unwrap();
        let chains = fl_chains(&crate::fl::song::read_song_with(&flp, false));
        assert!(chains.tracks.iter().any(|t| t == &["Fruity Reeverb 2".to_owned()]), "{chains:?}");
    }
}
