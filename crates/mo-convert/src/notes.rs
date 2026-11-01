//! What a converted song says it couldn't carry over, in the C# app's words, so both apps' notes read the same. The
//! DAWproject export will say the same things.

use mo_formats::fl::automation::{FlAutomation, FlControl};
use mo_formats::fl::song::{FlChannel, FlInsert, FlSong, FlTrack};

/// What's said of a channel whose FL volume envelope is switched on.
pub const ENVELOPE_LEFT_OUT: &str = "its volume envelope (ADSR) isn't carried over yet, so it plays without one";

/// What's said of a channel that the FL playlist never plays, carried as a track with no clips.
pub const PLAYS_NOTHING: &str =
    "the FL Studio playlist never plays it, so its track is here with its sound but no clips";

/// What's said of a channel switched off in FL's Channel Rack, whose `tracks` tracks are muted.
pub fn switched_off(tracks: usize) -> String {
    let which = if tracks > 1 { "its tracks are" } else { "its track is" };
    format!("switched off in FL Studio's Channel Rack, so {which} muted")
}

/// What's said of a channel whose on/off is automated ([`FlChannel::switched_by`]); `audio` when its audio clips are
/// carried.
pub fn switched_on_and_off(c: &FlChannel, audio: bool) -> String {
    let partly =
        if audio { c.audio_clips.iter().filter(|x| x.partly_off && !x.muted && !x.on_muted_track).count() } else { 0 };
    let by = c.switched_by.iter().map(|n| format!("\"{n}\"")).collect::<Vec<_>>().join(" and ");
    let rest = match partly {
        0 => String::new(),
        1 => "; one audio clip it switches off part of the way through plays whole".to_owned(),
        n => format!("; {n} audio clips it switches off part of the way through play whole"),
    };
    format!("switched on and off by {by} in FL Studio, so what it plays while it's off is muted{rest}")
}

/// How a note names an insert: "the master", "insert 4", or "Drums (insert 4)".
pub fn insert_label(insert: &FlInsert) -> String {
    if insert.index == 0 {
        "the master".to_owned()
    } else if insert.name.is_empty() {
        format!("insert {}", insert.index)
    } else {
        format!("{} (insert {})", insert.name, insert.index)
    }
}

/// How a note names an insert whose effects are copied: its name, or "Insert 4".
pub fn insert_name(insert: &FlInsert) -> String {
    if insert.name.is_empty() { format!("Insert {}", insert.index) } else { insert.name.clone() }
}

/// What's said of the mixer inserts (the master is insert 0) whose fader or pan isn't where FL leaves it: where it sits
/// isn't carried over yet (its automation is); `None` when none.
pub fn moved_inserts<'a>(inserts: impl IntoIterator<Item = &'a FlInsert>) -> Option<String> {
    let moved: Vec<String> = inserts.into_iter().filter(|i| i.moved()).map(insert_label).collect();
    (!moved.is_empty()).then(|| {
        format!(
            "Mixer: the fader or pan of {} isn't at FL Studio's default, and where a mixer fader or pan sits isn't carried \
             over yet",
            moved.join(", ")
        )
    })
}

/// Why an automation clip moving something no export carries yet is left out.
pub fn not_carried(a: &FlAutomation, song: &FlSong) -> String {
    let insert = match song.inserts.iter().find(|i| Some(i.index) == a.insert) {
        Some(found) => insert_label(found),
        None if a.insert == Some(0) => "the master".to_owned(),
        None => format!("insert {}", a.insert.map_or(-1, i64::from)),
    };
    match a.control {
        FlControl::MainVolume => "the master volume isn't automated here yet".to_owned(),
        FlControl::InsertVolume => format!("the fader of {insert} isn't automated here yet"),
        FlControl::InsertPan => format!("the pan of {insert} isn't automated here yet"),
        FlControl::Pitch => "pitch has no equivalent here".to_owned(),
        _ => "this control has no equivalent here".to_owned(),
    }
}

/// What's said of the patterns holding edit events (knob moves recorded into them) that move something other than a
/// plugin's setting, which aren't carried: up to
/// eight by name, then how many more. `None` when none.
pub fn edit_events(patterns: &[(u32, String)]) -> Option<String> {
    const SHOWN: usize = 8;
    let (first, more) = (patterns.iter().take(SHOWN), patterns.len().saturating_sub(SHOWN));
    let mut names = first.map(|(_, name)| format!("\"{name}\"")).collect::<Vec<_>>().join(", ");
    if more > 0 {
        names.push_str(&format!(" and {more} more"));
    }
    let s = if patterns.len() == 1 { "" } else { "s" };
    (!patterns.is_empty()).then(|| {
        format!(
            "Edit events: the knob moves recorded into pattern{s} {names} for anything but a plugin's setting aren't \
             carried over yet"
        )
    })
}

/// What's said of a track some of whose clips (`on`, by playlist track) sit on FL's muted playlist tracks: the track is
/// muted when they all do (`track_muted`), otherwise those clips are. `None` when none do.
pub fn on_muted_tracks(
    track: &str,
    on: impl IntoIterator<Item = u32>,
    muted: &[FlTrack],
    track_muted: bool,
) -> Option<String> {
    let numbers: std::collections::HashSet<u32> = on.into_iter().collect();
    let from: Vec<&FlTrack> = muted.iter().filter(|t| numbers.contains(&t.number)).collect();
    if from.is_empty() {
        return None;
    }
    let which = if from.len() == 1 { "a muted playlist track" } else { "muted playlist tracks" };
    let labels = from.iter().map(|t| t.label()).collect::<Vec<_>>().join("; ");
    let place = format!("{which} in FL Studio ({labels})");
    Some(if track_muted {
        format!("{track}: muted, because its clips are all on {place}")
    } else {
        format!("{track}: its clips on {place} are muted, as it also has clips from other playlist tracks")
    })
}

/// A number with up to `places` decimals and no trailing zeros, as C#'s "0.##" writes it.
pub fn number(v: f64, places: usize) -> String {
    let s = format!("{v:.places$}");
    let s = if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.') } else { &s };
    if s == "-0" { "0".to_owned() } else { s.to_owned() }
}

/// A number with its sign, as C#'s "+0.##;-0.##" writes it.
pub fn signed(v: f64) -> String {
    let n = number(v, 2);
    if n.starts_with('-') { n } else { format!("+{n}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_read_as_the_c_sharp_app_writes_them() {
        assert_eq!(number(140.0, 2), "140");
        assert_eq!(number(127.456, 2), "127.46");
        assert_eq!(number(-0.001, 2), "0");
        assert_eq!(signed(0.5), "+0.5");
        assert_eq!(signed(-1.25), "-1.25");
    }

    #[test]
    fn inserts_are_named_the_c_sharp_way() {
        let insert = |index, name: &str| FlInsert {
            index,
            name: name.into(),
            effects: Vec::new(),
            fader: Some(10_000),
            pan: None,
        };
        assert_eq!(insert_label(&insert(0, "Master")), "the master");
        assert_eq!(insert_label(&insert(4, "")), "insert 4");
        assert_eq!(insert_label(&insert(4, "Drums")), "Drums (insert 4)");
        assert_eq!(insert_name(&insert(4, "")), "Insert 4");
        assert_eq!(
            moved_inserts(&[insert(0, ""), insert(3, "Bass")]).unwrap(),
            "Mixer: the fader or pan of the master, Bass (insert 3) isn't at FL Studio's default, and where a mixer \
             fader or pan sits isn't carried over yet"
        );
    }
}
