//! What of a song you pick yourself for "Fits my song" to fit to: which of its parts, and which of its bars.
//!
//! Until you pick, the app does ([`super::fitted`]): the song's chords and bass, or its melodies, over the bars where
//! the most of them play together. A pick of yours is a [`YourPick`]: the parts by name ([`PartName`]: a song saved
//! again may have its parts in other places, but a part keeps its name), the bars by number. Either can be left to
//! the app.
//!
//! [`Picker`] is what the page shows to pick from, made by each search: every part of the song that can be fitted to
//! and whether it's on, the bars fitted to, and how long the song is. A press there changes the page's copy of it
//! ([`Picker::toggle`], [`Picker::set_bars`], [`Picker::apps_parts`], [`Picker::apps_pick`]), and [`Picker::pick`] is
//! then what the next search is asked for.
//!
//! [`YourPicks`] remembers the pick for each song, by the song library's key for it (so every save of the song has
//! it): the last [`PICKS_KEPT`] songs', in one setting ([`PICKS_KEY`]).

use std::collections::HashMap;
use std::ops::Range;

use mo_midi::Role;
use mo_midi::fit::for_song::Pick;
use mo_midi::playing::{Playing, Song};
use serde::{Deserialize, Serialize};

use crate::midi as page;

/// The most bars picked by hand: as far as two parts are heard going round together before they're taken to never
/// meet (`mo_harmony::round_of`).
pub const MOST_BARS: i64 = 32;

/// The setting your picks are remembered in ([`YourPicks`]).
pub const PICKS_KEY: &str = "midi_fits_picks";

/// How many songs' picks are remembered: the ones picked for last.
pub const PICKS_KEPT: usize = 32;

/// The most parts of one pick read back from the setting: a file on disk is never trusted to be small.
const PARTS_READ: usize = 256;

/// Whether a part of a song can be fitted to: it plays notes, and they're pitches (not a drum plugin's pads, nor a
/// part that plays as drums do).
pub fn can_fit(part: &Playing) -> bool {
    !part.drums && part.role != Role::Drums && !part.notes.is_empty()
}

/// One of a song's parts by what it keeps when the song is saved again: its name, and which of the parts of that
/// name it is (two channels called "Keys" are the first and the second "Keys").
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PartName {
    pub name: String,
    /// Which of the song's parts of this name, in the project's order, counted from 0.
    pub nth: usize,
}

impl PartName {
    /// The name of the part at `at` in [`Song::parts`]; none when the song has no such part.
    pub fn of(song: &Song, at: usize) -> Option<PartName> {
        let part = song.parts.get(at)?;
        let nth = song.parts.iter().take(at).filter(|p| p.name == part.name).count();
        Some(PartName { name: part.name.clone(), nth })
    }

    /// Where the part is in [`Song::parts`] now; none when the song has no such part any more.
    pub fn place(&self, song: &Song) -> Option<usize> {
        song.parts.iter().enumerate().filter(|(_, p)| p.name == self.name).nth(self.nth).map(|(at, _)| at)
    }
}

/// What of a song you picked by hand. What isn't picked is the app's to pick ([`super::fitted`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct YourPick {
    /// The parts; none: the app picks them.
    pub parts: Option<Vec<PartName>>,
    /// The bars, counted from 0; none: the app picks them.
    pub bars: Option<Range<i64>>,
}

impl YourPick {
    /// Whether nothing is picked by hand.
    pub fn is_empty(&self) -> bool {
        self.parts.is_none() && self.bars.is_none()
    }

    /// The pick as `song` has it now: the parts' places in [`Song::parts`], in the song's order, and the bars. A part
    /// the song has no more (renamed or deleted since) or that can't be fitted to ([`can_fit`]) drops out, and with
    /// none left the parts are the app's to pick again. Bars past the song's end are cut off ([`held_bars`]), and
    /// bars that start past it are the app's to pick again.
    pub fn in_song(&self, song: &Song) -> (Option<Vec<usize>>, Option<Range<i64>>) {
        let fits = |at: &usize| song.parts.get(*at).is_some_and(can_fit);
        let parts = self.parts.as_ref().and_then(|names| {
            let mut places: Vec<usize> = names.iter().filter_map(|name| name.place(song)).filter(fits).collect();
            places.sort_unstable();
            places.dedup();
            (!places.is_empty()).then_some(places)
        });
        let length = song.bars();
        let bars = self.bars.as_ref().filter(|bars| bars.start < length).and_then(|bars| held_bars(bars, length));
        (parts, bars)
    }
}

/// Bars picked by hand (counted from 0), held to a song of `length` bars: they start in the song (its last bar at
/// the latest), are a bar long at the least and [`MOST_BARS`] at the most, and end where the song does at the
/// latest. None for a song with no bars.
pub fn held_bars(bars: &Range<i64>, length: i64) -> Option<Range<i64>> {
    if length <= 0 {
        return None;
    }
    let start = bars.start.clamp(0, length - 1);
    let end = bars.end.clamp(start + 1, length).min(start.saturating_add(MOST_BARS));
    Some(start..end)
}

/// One part of a song in the page's picker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PartChoice {
    pub part: PartName,
    /// What its chip says: its name, with "(2)" after it for the second part of that name.
    pub label: String,
    /// The kind of part the app takes it for ("Chords", "Bassline", "Melody"), or "Sample" for a sampler.
    pub kind: &'static str,
    /// A sampler (`mo_midi::playing::Playing::sample`): the app never picks one, and picked by hand its notes are
    /// taken as written, which is right when the sample is tuned to the keys.
    pub sample: bool,
    /// The bars it plays in, counted from 0.
    pub bars: Range<i64>,
    /// Whether it's one of the parts picked: by you, else by the app.
    pub on: bool,
}

/// What the page shows to pick a song's parts and bars from, as one search left them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Picker {
    /// Every part of the song that can be fitted to ([`can_fit`]), in the project's order.
    pub parts: Vec<PartChoice>,
    /// The bars fitted to, counted from 0: yours, else the app's. None when the app found none to pick.
    pub bars: Option<Range<i64>>,
    /// How many bars the song's notes run.
    pub length: i64,
    /// Whether the parts that are on, and the bars, are ones you picked (so the app's pick is worth offering back).
    pub your_parts: bool,
    pub your_bars: bool,
}

impl Picker {
    /// The picker for `song` after a search asked for `yours`: `fitted` is what the list was fitted to, none when
    /// nothing could be. The parts that are on are yours (every one the song still has, also one that rests through
    /// the bars and so isn't heard), else the app's.
    pub fn of(song: &Song, yours: &YourPick, fitted: Option<&Pick>) -> Picker {
        let (your_parts, your_bars) = yours.in_song(song);
        let on: &[usize] = match (&your_parts, fitted) {
            (Some(parts), _) => parts,
            (None, Some(pick)) => &pick.parts,
            (None, None) => &[],
        };
        // Which of its name each part is, counted as [`PartName::of`] counts: among every part of the song.
        let mut seen: HashMap<&str, usize> = HashMap::new();
        let mut parts = Vec::new();
        for (at, part) in song.parts.iter().enumerate() {
            let count = seen.entry(part.name.as_str()).or_insert(0);
            let nth = *count;
            *count += 1;
            if !can_fit(part) {
                continue;
            }
            let name = if part.name.is_empty() { "No name" } else { part.name.as_str() };
            parts.push(PartChoice {
                part: PartName { name: part.name.clone(), nth },
                label: if nth == 0 { name.to_owned() } else { format!("{name} ({})", nth + 1) },
                kind: if part.sample { "Sample" } else { page::role_label(part.role) },
                sample: part.sample,
                bars: part.bars.clone(),
                on: on.contains(&at),
            });
        }
        Picker {
            parts,
            bars: fitted.map(|pick| pick.bars.clone()).or_else(|| your_bars.clone()),
            length: song.bars(),
            your_parts: your_parts.is_some(),
            your_bars: your_bars.is_some(),
        }
    }

    /// What's picked by hand as the picker stands: what the next search is asked for, and what's remembered for the
    /// song.
    pub fn pick(&self) -> YourPick {
        let on: Vec<PartName> = self.parts.iter().filter(|p| p.on).map(|p| p.part.clone()).collect();
        YourPick {
            parts: (self.your_parts && !on.is_empty()).then_some(on),
            bars: self.bars.clone().filter(|_| self.your_bars),
        }
    }

    /// Whether any of it is yours.
    pub fn is_yours(&self) -> bool {
        self.your_parts || self.your_bars
    }

    /// A press on the chip of the part at `at` (a place in [`Picker::parts`]): it goes on or off, and the parts that
    /// are on are yours from here. With the last one turned off the app picks the parts again (which, the next
    /// search's picker says).
    pub fn toggle(&mut self, at: usize) {
        let Some(part) = self.parts.get_mut(at) else { return };
        part.on = !part.on;
        self.your_parts = self.parts.iter().any(|p| p.on);
    }

    /// The bars set by hand, counted from 0, held to the song ([`held_bars`]): they're yours from here.
    pub fn set_bars(&mut self, bars: Range<i64>) {
        if let Some(bars) = held_bars(&bars, self.length) {
            self.bars = Some(bars);
            self.your_bars = true;
        }
    }

    /// The parts are the app's to pick again: the page's switch for what's looked for was pressed, which is the
    /// app's pick of the song's chords, or of its melodies. Bars of yours stay. Which parts are on, the next search's
    /// picker says.
    pub fn apps_parts(&mut self) {
        self.your_parts = false;
    }

    /// Everything is the app's to pick again.
    pub fn apps_pick(&mut self) {
        self.your_parts = false;
        self.your_bars = false;
    }
}

/// Your picks, remembered: one for each of the songs picked for last ([`PICKS_KEPT`] of them), the newest first,
/// each under the song library's key for its song (`mo_library::Song::key`: the same for every save of it).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct YourPicks(Vec<(String, YourPick)>);

/// One pick as the setting holds it.
#[derive(Serialize, Deserialize)]
struct KeptPick {
    song: String,
    /// Each part's name and which of that name it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parts: Option<Vec<(String, usize)>>,
    /// The first bar and the bar after the last, counted from 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bars: Option<(i64, i64)>,
}

impl YourPicks {
    /// The picks as the setting has them (`text`: what [`PICKS_KEY`] holds). A setting that can't be read is no
    /// picks: the app picks again, as it does for a song not picked for yet.
    pub fn read(text: &str) -> YourPicks {
        let kept: Vec<KeptPick> = serde_json::from_str(text).unwrap_or_default();
        let mut picks: Vec<(String, YourPick)> = Vec::new();
        for kept in kept {
            if picks.len() >= PICKS_KEPT {
                break;
            }
            let named = |(name, nth)| PartName { name, nth };
            let parts: Option<Vec<PartName>> =
                kept.parts.map(|parts| parts.into_iter().take(PARTS_READ).map(named).collect());
            let pick = YourPick {
                parts: parts.filter(|parts| !parts.is_empty()),
                bars: kept.bars.filter(|(start, end)| *start >= 0 && end > start).map(|(start, end)| start..end),
            };
            if !kept.song.is_empty() && !pick.is_empty() && !picks.iter().any(|(song, _)| *song == kept.song) {
                picks.push((kept.song, pick));
            }
        }
        YourPicks(picks)
    }

    /// The picks as the setting holds them ([`YourPicks::read`] reads them back).
    pub fn text(&self) -> String {
        let kept: Vec<KeptPick> = self
            .0
            .iter()
            .map(|(song, pick)| KeptPick {
                song: song.clone(),
                parts: pick.parts.as_ref().map(|parts| parts.iter().map(|p| (p.name.clone(), p.nth)).collect()),
                bars: pick.bars.as_ref().map(|bars| (bars.start, bars.end)),
            })
            .collect();
        serde_json::to_string(&kept).unwrap_or_else(|_| "[]".to_owned())
    }

    /// What you picked for the song with this key; nothing when you haven't.
    pub fn of(&self, song: &str) -> YourPick {
        self.0.iter().find(|(key, _)| key == song).map(|(_, pick)| pick.clone()).unwrap_or_default()
    }

    /// How many songs have a pick remembered.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Remembers `pick` for the song with this key, as the one picked for last; a pick with nothing by hand forgets
    /// the song's. The oldest goes when there are more than [`PICKS_KEPT`]. Whether anything changed, so the setting
    /// is only written when it did.
    pub fn set(&mut self, song: &str, pick: &YourPick) -> bool {
        if song.is_empty() {
            return false;
        }
        let at = self.0.iter().position(|(key, _)| key == song);
        if pick.is_empty() {
            return at.map(|at| self.0.remove(at)).is_some();
        }
        if at == Some(0) && self.0.first().is_some_and(|(_, was)| was == pick) {
            return false;
        }
        if let Some(at) = at {
            self.0.remove(at);
        }
        self.0.insert(0, (song.to_owned(), pick.clone()));
        self.0.truncate(PICKS_KEPT);
        true
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use mo_midi::fit::Wants;

    use super::super::tests::{chords, lane, song, song_of, tune};
    use super::super::{fitted, fitted_text};
    use super::*;

    fn named(name: &str, nth: usize) -> PartName {
        PartName { name: name.into(), nth }
    }

    fn parts(names: &[&str]) -> YourPick {
        YourPick { parts: Some(names.iter().map(|name| named(name, 0)).collect()), bars: None }
    }

    #[test]
    fn a_part_is_known_by_its_name_and_which_of_that_name_it_is() {
        // Two channels called "Keys" (a layered sound) around a lead.
        let song = song_of(vec![lane("Keys", 0, &chords()), lane("Lead", 0, &tune(0)), lane("Keys", 4, &chords())]);
        assert_eq!(PartName::of(&song, 0), Some(named("Keys", 0)));
        assert_eq!(PartName::of(&song, 1), Some(named("Lead", 0)));
        assert_eq!(PartName::of(&song, 2), Some(named("Keys", 1)));
        assert_eq!(PartName::of(&song, 3), None);
        for at in 0..3 {
            assert_eq!(PartName::of(&song, at).unwrap().place(&song), Some(at));
        }
        // Saved again with the lead first: each name still finds its part. A name the song has no more finds none.
        let moved = song_of(vec![lane("Lead", 0, &tune(0)), lane("Keys", 0, &chords()), lane("Keys", 4, &chords())]);
        assert_eq!(named("Lead", 0).place(&moved), Some(0));
        assert_eq!(named("Keys", 1).place(&moved), Some(2));
        assert_eq!(named("Keys", 2).place(&moved), None);
        assert_eq!(named("Pad", 0).place(&moved), None);
    }

    #[test]
    fn parts_picked_by_hand_are_what_the_list_is_fitted_to() {
        // Chords in bars 9 to 12, a lead over the next four.
        let song = song();
        let fit = |yours: &YourPick, wanted| fitted(&song, wanted, yours).map(|f| fitted_text(&song, &f));
        // The lead alone: chords for it, whatever the switch asked for last (what the parts are decides).
        assert_eq!(fit(&parts(&["Lead"]), None).unwrap(), "Chords for Lead, bars 13–16");
        assert_eq!(fit(&parts(&["Lead"]), Some(Wants::Tune)).unwrap(), "Chords for Lead, bars 13–16");
        // Both: something to play over the two, from where the first of them comes in.
        let both = fitted(&song, None, &parts(&["Lead", "Keys"])).unwrap();
        assert_eq!((both.wants, both.pick.parts.clone(), both.pick.bars.clone()), (Wants::Tune, vec![0, 1], 8..16));
        assert_eq!(fitted_text(&song, &both), "To play over Keys and Lead, bars 9–16");
        // What a part found is heard with is the same parts and bars.
        let heard: Vec<&str> = both.with.voices.iter().map(|v| v.name.as_str()).collect();
        assert_eq!((heard, both.with.bars.clone()), (vec!["Keys", "Lead"], 8..16));

        // A part the song has no more drops out, and with none left the app picks, as if nothing were picked.
        assert_eq!(fit(&parts(&["Pad", "Lead"]), None).unwrap(), "Chords for Lead, bars 13–16");
        assert_eq!(fitted(&song, None, &parts(&["Pad"])), fitted(&song, None, &YourPick::default()));
        // Drums can't be picked: their notes pick pads.
        let mut with_drums = song.clone();
        with_drums.parts[0].drums = true;
        assert_eq!(parts(&["Keys", "Lead"]).in_song(&with_drums), (Some(vec![1]), None));
        // A sampler can, by hand only: its notes are taken as written.
        let mut sampled = song.clone();
        sampled.parts[0].sample = true;
        assert_eq!(fitted(&sampled, None, &YourPick::default()).unwrap().wants, Wants::Chords);
        let by_hand = fitted(&sampled, None, &parts(&["Keys"])).unwrap();
        assert_eq!(fitted_text(&sampled, &by_hand), "To play over Keys, bars 9–12");
    }

    #[test]
    fn bars_picked_by_hand_are_the_bars_the_list_is_fitted_to() {
        let song = song();
        let bars = |bars: Range<i64>| YourPick { parts: None, bars: Some(bars) };
        let fit = |yours: &YourPick, wanted| fitted(&song, wanted, yours).map(|f| fitted_text(&song, &f));
        // The app's parts over your bars: the chords where you say.
        assert_eq!(fit(&bars(8..10), None).unwrap(), "To play over Keys, bars 9–10");
        // Where only the lead plays, it's the lead that's fitted to; asked for parts over the chords, why not.
        assert_eq!(fit(&bars(12..16), None).unwrap(), "Chords for Lead, bars 13–16");
        assert_eq!(
            fit(&bars(12..16), Some(Wants::Tune)).unwrap_err(),
            "This song's chords and basslines don't play in bars 13–16. A melody plays there: look for chords for \
             that."
        );
        assert_eq!(
            fit(&bars(8..12), Some(Wants::Chords)).unwrap_err(),
            "This song's melodies don't play in bars 9–12. Chords play there: look for parts to play over them."
        );
        // Bars nothing plays in.
        assert_eq!(
            fit(&bars(0..4), None).unwrap_err(),
            "This song's chords, basslines and melodies don't play in bars 1–4."
        );
        assert_eq!(
            fit(&bars(0..4), Some(Wants::Tune)).unwrap_err(),
            "This song's chords and basslines don't play in bars 1–4."
        );
        // A song with no melody at all says that, whatever the bars.
        let keys_only = song_of(vec![lane("Keys", 8, &chords())]);
        assert_eq!(
            fitted(&keys_only, Some(Wants::Chords), &bars(0..4)).unwrap_err(),
            "No melody found in this song to put chords under."
        );

        // Bars and parts: the parts over those bars, and why not when they rest there.
        let both = YourPick { bars: Some(14..16), ..parts(&["Lead"]) };
        assert_eq!(fit(&both, None).unwrap(), "Chords for Lead, bars 15–16");
        let rests = YourPick { bars: Some(8..12), ..parts(&["Lead"]) };
        assert_eq!(fit(&rests, None).unwrap_err(), "Lead doesn't play in bars 9–12.");
        let rest = YourPick { bars: Some(0..4), ..parts(&["Keys", "Lead"]) };
        assert_eq!(fit(&rest, None).unwrap_err(), "Keys and Lead don't play in bars 1–4.");
        // Of two picked, the one that plays there is what's fitted to and named.
        let one = YourPick { bars: Some(8..12), ..parts(&["Keys", "Lead"]) };
        assert_eq!(fit(&one, None).unwrap(), "To play over Keys, bars 9–12");

        // Bars are held to the song (16 bars here): cut off at its end, and left to the app when they start past it.
        assert_eq!(fit(&bars(14..99), None).unwrap(), "Chords for Lead, bars 15–16");
        assert_eq!(fitted(&song, None, &bars(40..48)), fitted(&song, None, &YourPick::default()));
        assert_eq!(held_bars(&(-3..2), 16), Some(0..2));
        assert_eq!(held_bars(&(99..120), 16), Some(15..16));
        assert_eq!(held_bars(&(5..5), 16), Some(5..6));
        assert_eq!(held_bars(&(0..100), 64), Some(0..MOST_BARS));
        assert_eq!(held_bars(&(0..4), 0), None);
    }

    #[test]
    fn the_picker_shows_what_is_picked_and_a_press_changes_it() {
        // Chords in bars 9 to 12, a lead over the next four, and drums, which can't be fitted to.
        let mut song = song_of(vec![lane("Keys", 8, &chords()), lane("Lead", 12, &tune(0)), lane("Kit", 8, &chords())]);
        song.parts[2].drums = true;
        // Nothing picked: the app's parts are on, and nothing is yours.
        let apps = fitted(&song, None, &YourPick::default()).unwrap();
        let mut picker = Picker::of(&song, &YourPick::default(), Some(&apps.pick));
        let shown = |p: &Picker| p.parts.iter().map(|c| (c.label.clone(), c.kind, c.on)).collect::<Vec<_>>();
        assert_eq!(shown(&picker), [("Keys".to_owned(), "Chords", true), ("Lead".to_owned(), "Melody", false)]);
        assert_eq!(picker.parts[1].bars, 12..16);
        assert_eq!((picker.bars.clone(), picker.length, picker.is_yours()), (Some(8..12), 16, false));
        assert_eq!(picker.pick(), YourPick::default());

        // The lead turned on: the parts are yours, both of them, and the bars still the app's.
        picker.toggle(1);
        assert_eq!(picker.pick(), parts(&["Keys", "Lead"]));
        // The keys turned off, and the bars set: held to the song.
        picker.toggle(0);
        picker.set_bars(13..99);
        let yours = picker.pick();
        assert_eq!(yours, YourPick { bars: Some(13..16), ..parts(&["Lead"]) });
        // The search asked for that gives the picker back as it was left.
        let by_hand = fitted(&song, None, &yours).unwrap();
        let after = Picker::of(&song, &yours, Some(&by_hand.pick));
        assert_eq!(after, picker);
        assert!(after.your_parts && after.your_bars);

        // The switch pressed: the parts are the app's again, the bars stay yours.
        picker.apps_parts();
        assert_eq!(picker.pick(), YourPick { parts: None, bars: Some(13..16) });
        // The last part turned off: the app picks the parts again.
        let mut none_on = after.clone();
        none_on.toggle(1);
        assert_eq!(none_on.pick(), YourPick { parts: None, bars: Some(13..16) });
        // The app's pick back, all of it.
        picker.apps_pick();
        assert_eq!((picker.pick(), picker.is_yours()), (YourPick::default(), false));
        // A press on a chip that isn't there, and bars for a song with none, change nothing.
        let mut same = after.clone();
        same.toggle(9);
        assert_eq!(same, after);
        let mut empty = Picker::default();
        empty.set_bars(0..4);
        assert_eq!(empty, Picker::default());

        // Nothing could be fitted (the lead rests in your bars): the picker still shows your pick, to change it.
        let rests = YourPick { bars: Some(8..12), ..parts(&["Lead"]) };
        assert!(fitted(&song, None, &rests).is_err());
        let stuck = Picker::of(&song, &rests, None);
        assert_eq!(shown(&stuck), [("Keys".to_owned(), "Chords", false), ("Lead".to_owned(), "Melody", true)]);
        assert_eq!((stuck.bars.clone(), stuck.pick()), (Some(8..12), rests));

        // Two parts of one name are told apart, a part with no name has words, and a sampler says what it is.
        let mut twins = song_of(vec![lane("Keys", 0, &chords()), lane("Keys", 0, &chords()), lane("", 0, &tune(0))]);
        twins.parts[1].sample = true;
        let picker = Picker::of(&twins, &YourPick { parts: Some(vec![named("Keys", 1)]), bars: None }, None);
        assert_eq!(
            shown(&picker),
            [
                ("Keys".to_owned(), "Chords", false),
                ("Keys (2)".to_owned(), "Sample", true),
                ("No name".to_owned(), "Melody", false)
            ]
        );
        assert_eq!(picker.pick().parts, Some(vec![named("Keys", 1)]));
    }

    #[test]
    fn picks_are_remembered_for_the_songs_picked_for_last() {
        let lead = YourPick { bars: Some(12..16), ..parts(&["Lead"]) };
        let keys = YourPick { parts: Some(vec![named("Keys", 0), named("Keys", 1)]), bars: None };
        let mut picks = YourPicks::default();
        assert!(picks.set("night drive", &lead));
        assert!(picks.set("glass", &keys));
        // Remembered as they are: nothing to write again.
        assert!(!picks.set("glass", &keys));
        assert_eq!(
            (picks.of("night drive"), picks.of("glass"), picks.of("other")),
            (lead.clone(), keys.clone(), YourPick::default())
        );
        // The setting holds them, the newest first, and gives them back.
        let text = picks.text();
        assert_eq!(
            text,
            r#"[{"song":"glass","parts":[["Keys",0],["Keys",1]]},{"song":"night drive","parts":[["Lead",0]],"bars":[12,16]}]"#
        );
        assert_eq!(YourPicks::read(&text), picks);
        // A song picked for again is the newest; one with nothing by hand any more is forgotten.
        assert!(picks.set("night drive", &lead));
        assert!(picks.text().starts_with(r#"[{"song":"night drive""#));
        assert!(picks.set("glass", &YourPick::default()));
        assert!(!picks.set("glass", &YourPick::default()));
        assert_eq!((picks.len(), picks.of("glass")), (1, YourPick::default()));
        // A song with no key has nowhere to be remembered.
        assert!(!picks.set("", &lead));

        // Only the last few songs' are kept.
        let mut many = YourPicks::default();
        for n in 0..PICKS_KEPT + 5 {
            many.set(&format!("song {n}"), &lead);
        }
        assert_eq!(many.len(), PICKS_KEPT);
        assert_eq!((many.of("song 4"), many.of("song 5")), (YourPick::default(), lead.clone()));
        assert_eq!(YourPicks::read(&many.text()), many);

        // A setting that can't be read, or holds what's no pick, is no picks.
        assert!(YourPicks::read("").is_empty());
        assert!(YourPicks::read("{not json").is_empty());
        let odd = r#"[{"song":"","parts":[["Lead",0]]},{"song":"a"},{"song":"b","bars":[8,8]},{"song":"c","parts":[]},
                      {"song":"d","bars":[-4,2]},{"song":"e","bars":[4,6]},{"song":"e","bars":[1,2]}]"#;
        let read = YourPicks::read(odd);
        assert_eq!((read.len(), read.of("e")), (1, YourPick { parts: None, bars: Some(4..6) }));
    }
}
