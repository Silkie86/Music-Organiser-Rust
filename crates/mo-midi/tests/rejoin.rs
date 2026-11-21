//! An FL Studio song's phrases joined back from its channels (`mo_midi::rejoin`), read from a whole save the way the
//! indexer reads it (`mo_midi::extract`), on made-up FL Studio projects.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

#[allow(unused_imports)]
#[path = "../../mo-formats/tests/common/mod.rs"]
mod common;

use std::path::Path;

use common::*;
use mo_midi::extract::{self, Found};

/// A quarter note in the made-up saves (their ticks are the index's own).
const BEAT: u32 = 96;

/// A song whose lead is `bars` bars on Synth A (the pattern "Lead A") answered by as many on Synth B ("Lead B"), from
/// the song's bar 17, `times` times with 16 bars between. `beats` is how many quarter notes its bars have; with
/// three it's saved as a song in 3/4.
fn handed_lead(beats: u32, bars: u32, times: u32) -> Vec<u8> {
    handed_lead_from(beats, bars, times, 16, 0)
}

/// [`handed_lead`] with its first clip on the song's bar `first` (from 0), and Synth A's pattern resting for its
/// first `rest` bars.
fn handed_lead_from(beats: u32, bars: u32, times: u32, first: u32, rest: u32) -> Vec<u8> {
    let tune = |channel: u16, low: u16, rest: u32| -> Vec<u8> {
        (rest * beats..bars * beats)
            .flat_map(|at| note(at * BEAT, channel, BEAT, low + (at % beats) as u16 * 2, 100))
            .collect()
    };
    let (bar, each) = (beats * BEAT, bars * beats * BEAT);
    let playlist: Vec<u8> = (0..times)
        .flat_map(|time| {
            let from = (first + time * (2 * bars + 16)) * bar;
            [playlist_item(from, 1, each, -1), playlist_item(from + each, 2, each, -1)].concat()
        })
        .collect();
    let song = Flp::new().ascii(199, "21.2.3.4004").dword(156, 120_000);
    let song = if beats == 4 { song } else { song.byte(17, beats as u8) };
    song.word(64, 0)
        .byte(21, 2)
        .text(201, "Sytrus")
        .text(203, "Synth A")
        .data(213, &[9])
        .word(64, 1)
        .byte(21, 2)
        .text(201, "3x Osc")
        .text(203, "Synth B")
        .data(213, &[9])
        .word(65, 1)
        .text(193, "Lead A")
        .data(224, &tune(0, 60, rest))
        .word(65, 2)
        .text(193, "Lead B")
        .data(224, &tune(1, 62, 0))
        .data(233, &playlist)
        .build()
}

fn read(bytes: &[u8]) -> Vec<Found> {
    extract::from_bytes(Path::new("song.flp"), bytes, &[]).unwrap()
}

/// Each part found, as the page lists it: its name and its clip line.
fn listed(found: &[Found]) -> Vec<(&str, &str)> {
    found.iter().map(|f| (f.part.as_str(), f.clip.as_str())).collect()
}

/// The 19 November 2026 review's MP-8: a waltz's lead from the song's bar 17 was "from bar 13" (17 bars of 3/4 in are
/// 13 of 4/4), and its 16 bars were counted as 12, an untidy length: played only twice it wasn't joined at all.
#[test]
fn a_waltzs_handed_lead_is_from_the_bar_the_playlist_shows() {
    let found = read(&handed_lead(3, 8, 2));
    assert_eq!(
        listed(&found),
        [("Synth A", "Lead A"), ("Synth B", "Lead B"), ("Synth A + Synth B", "From bar 17 of the song, twice")]
    );
    // The whole phrase, uncut: 16 bars of the waltz, 12 bars as the library counts every part (in 4/4).
    let whole = &found[2];
    assert_eq!((whole.notes.len(), whole.notes[0].start, whole.analysis.bars, whole.cut), (48, 0, 12, false));
    // Its pieces are the two patterns as they list on their own, so each one's card can show the whole phrase.
    let pieces: Vec<&str> = whole.members.iter().map(|m| m.hash.as_str()).collect();
    assert_eq!(pieces, [found[0].analysis.hash.as_str(), found[1].analysis.hash.as_str()]);
    let (half, bar) = (i64::from(8 * 3 * BEAT), i64::from(3 * BEAT));
    assert_eq!(whole.members.iter().map(|m| (m.start, m.length)).collect::<Vec<_>>(), [(0, half), (half, half)]);
    assert_eq!(half, 8 * bar);

    // Three times round it was joined before too, but called "From bar 13".
    let found = read(&handed_lead(3, 8, 3));
    assert_eq!(listed(&found)[2], ("Synth A + Synth B", "From bar 17 of the song, 3 times"));
}

/// A piece of a joined phrase is known by the hash its pattern lists under, so the pattern's card can show the whole
/// phrase: on whichever of the waltz's bars the clip sits, and where the pattern rests for its first bar.
#[test]
fn a_waltzs_pieces_are_the_patterns_as_they_list_wherever_the_clips_sit() {
    let pieces_are_the_patterns = |found: &[Found]| {
        let pieces: Vec<&str> = found[2].members.iter().map(|m| m.hash.as_str()).collect();
        pieces == [found[0].analysis.hash.as_str(), found[1].analysis.hash.as_str()]
    };
    // Clips on the song's bars 17 to 20: each sits on another beat of a bar of 4/4.
    for first in 16..20 {
        let found = read(&handed_lead_from(3, 8, 3, first, 0));
        let from = format!("From bar {} of the song, 3 times", first + 1);
        assert_eq!(listed(&found)[2], ("Synth A + Synth B", from.as_str()));
        assert!(pieces_are_the_patterns(&found), "clips from bar {}", first + 1);
        // Synth A's pattern rests for a bar first: the phrase is from the bar after the clip's.
        let found = read(&handed_lead_from(3, 8, 3, first, 1));
        let from = format!("From bar {} of the song, 3 times", first + 2);
        assert_eq!(listed(&found)[2], ("Synth A + Synth B", from.as_str()));
        assert_eq!(found[0].notes[0].start, i64::from(3 * BEAT));
        assert!(pieces_are_the_patterns(&found), "clips from bar {}, a bar's rest first", first + 1);
    }
    // The same in 5/4, where a bar's rest is longer than a bar of 4/4.
    let found = read(&handed_lead_from(5, 4, 3, 16, 1));
    assert_eq!(listed(&found)[2], ("Synth A + Synth B", "From bar 18 of the song, 3 times"));
    assert_eq!(found[0].notes[0].start, i64::from(BEAT));
    assert!(pieces_are_the_patterns(&found));
}

/// The same lead in 4/4 reads as it always did: bars of the song are bars of 4/4 there.
#[test]
fn a_song_in_four_four_joins_as_before() {
    let found = read(&handed_lead(4, 8, 2));
    assert_eq!(listed(&found)[2], ("Synth A + Synth B", "From bar 17 of the song, twice"));
    assert_eq!((found[2].notes.len(), found[2].analysis.bars, found[2].members.len()), (64, 16, 2));
}

/// In a song with very long bars a joined phrase can hold a rest long enough for the index to cut it after all. The
/// piece of it that's listed is then not what the phrase's pieces are of, and isn't given them.
#[test]
fn a_joined_phrase_cut_again_keeps_no_pieces() {
    // 16/4: a note on Synth A at the start of a bar, answered by Synth B in the last four beats of the next bar,
    // twice. Synth A's clip is cut short (its pattern goes on), so its note alone is no pattern of the song.
    let bar = 16 * BEAT;
    let pickup = [note(0, 0, BEAT, 60, 100), note(8 * BEAT, 0, BEAT, 64, 100)].concat();
    let answer = [note(0, 1, BEAT, 62, 100), note(BEAT, 1, BEAT, 65, 100), note(2 * BEAT, 1, BEAT, 67, 100)].concat();
    let playlist: Vec<u8> = (0..2)
        .flat_map(|time| {
            let from = (4 + time * 8) * bar;
            [playlist_item(from, 1, BEAT, -1), playlist_item(from + 2 * bar - 4 * BEAT, 2, 4 * BEAT, -1)].concat()
        })
        .collect();
    let bytes = Flp::new()
        .ascii(199, "21.2.3.4004")
        .dword(156, 120_000)
        .byte(17, 16)
        .word(64, 0)
        .byte(21, 2)
        .text(201, "Sytrus")
        .text(203, "Synth A")
        .data(213, &[9])
        .word(64, 1)
        .byte(21, 2)
        .text(201, "3x Osc")
        .text(203, "Synth B")
        .data(213, &[9])
        .word(65, 1)
        .text(193, "Pickup")
        .data(224, &pickup)
        .word(65, 2)
        .text(193, "Answer")
        .data(224, &answer)
        .data(233, &playlist)
        .build();
    let found = read(&bytes);
    // The phrase is the note and its answer seven bars of 4/4 on: cut in two, and the answer is the pattern already
    // listed. So one piece of it is listed, the note alone.
    assert_eq!(
        listed(&found),
        [("Synth A", "Pickup"), ("Synth B", "Answer"), ("Synth A + Synth B", "From bar 5 of the song, twice · bar 1")]
    );
    assert_eq!(found[2].notes.len(), 1);
    assert_eq!(found[2].members, []);
}
