//! Put in the song: a MIDI part goes into Bitwig's arranger at the bars of the song it was found for, on the track
//! selected there.
//!
//! Bitwig's controller API can put a clip in a clip launcher slot, but not on the arranger's timeline. So the part
//! goes in the way a person gets a launcher clip into the arranger: Bitwig records it there. The part goes in a
//! launcher slot of the selected track, the extension's `record` job plays it through once from the first of the
//! bars with arranger record on (which lays the clip on the track's timeline), and the launcher clip comes out again.
//! The job sets Bitwig up so only that clip records and nothing is heard, and puts every setting back
//! ([`Link::record_clip`]). It takes as long as the part does to play.
//!
//! What it records over is replaced, as when a clip is dropped there: the bars on that track are the part's
//! afterwards. Bitwig's undo takes it out again.
//!
//! Only in the project the bars were read from (by its name, which is all Bitwig tells the link: see
//! [`Bitwig::play_with_song`]), on an instrument track, and with Bitwig stopped. Needs the extension 3.19 or later
//! (`selectedtrack`, a `clip` noted as the app's, `record` from 3.9) and "Let the app make changes" on.
//!
//! The record job was tried live in the C# app's test project on Bitwig 6.1.5 (October 2026: the launched clip is
//! recorded, armed or not, the muted master leaves it alone, and the take isn't an alias of the launcher clip). In a
//! real song it's untried: `extensions/bitwig-java/README.md` lists what only a real Bitwig can show.

use std::path::Path;
use std::time::{Duration, Instant};

use super::audition::{check_version, same_project, there};
use super::link::Recorded;
use super::send::{Turn, sentence};
use super::{Bitwig, Error, Link, SongLoop};

/// The extension version Put in the song needs.
pub const PUT_NEEDS: &str = "3.19";

/// How long a Bitwig the app has just stopped gets to show it has, before the part is put in.
const STOP_WAIT: Duration = Duration::from_secs(2);

/// The tempo the wait is worked out at when Bitwig doesn't say its own, in BPM: a slow one, so the app never gives
/// up on a recording still under way.
const SLOW_TEMPO: f64 = 60.0;

/// A part put into its song's bars in Bitwig ([`Bitwig::put_in_song`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PutIn {
    /// The track it went on, as Bitwig showed its name.
    pub track: String,
    /// How much of it the link saw go in.
    pub seen: Seen,
    /// The settings Bitwig didn't show back as they were before the recording, by the link's names ("master mute",
    /// "pre-roll"): worth a look there.
    pub not_back: Vec<String>,
    /// Whether the clip the part was recorded from may still be in the track's clip launcher (it couldn't be taken
    /// out).
    pub clip_left: bool,
}

/// What the link saw of a part as Bitwig recorded it ([`PutIn::seen`]). The link can't look into the arranger, so
/// this is what it saw play while record was on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seen {
    /// The part's clip played to its end.
    Whole,
    /// Bitwig stopped, or the time ran out, before the part's end: its last bars may be missing.
    Short,
    /// Bitwig played, but the link didn't see the part's clip play: only a look at Bitwig says what went in.
    Unseen,
}

impl Bitwig {
    /// Puts the MIDI file at `path` into Bitwig's arranger on the selected track: from `bars.start` beats for
    /// `bars.length` beats (whole bars of the song; a part shorter than that goes round until they're full), by
    /// having Bitwig record it from the track's clip launcher. Bitwig has read the file once the recording starts,
    /// and this returns when it's over, so the caller may delete the file then.
    ///
    /// Fails with nothing changed when Bitwig has another project open than `bars.project`, no instrument track is
    /// selected, Bitwig is playing, the extension is older than 3.19, or changes are off. When the recording itself
    /// doesn't go through, the error says what Bitwig did and what may be left there.
    ///
    /// It goes in `_turn` ([`super::ticket`]), behind the sends, plays and stops asked for before it (so a part the
    /// app was playing there has stopped). `wanted` is asked as its turn comes and again before the recording starts:
    /// one no longer wanted (the app is closing) puts nothing in ([`Error::Cancelled`]).
    pub fn put_in_song(
        &self,
        path: impl AsRef<Path>,
        bars: &SongLoop,
        _turn: &Turn<'_>,
        wanted: &dyn Fn() -> bool,
    ) -> Result<PutIn, Error> {
        let path = path.as_ref();
        if !wanted() {
            return Err(Error::Cancelled);
        }
        if !(bars.start.is_finite() && bars.start >= 0.0 && bars.length.is_finite() && bars.length > 0.0) {
            return Err(Error::Invalid(format!(
                "The song's bars start at beat {} and are {} beats long, which Bitwig can't record into.",
                bars.start, bars.length
            )));
        }
        let mut link = self.connect()?;
        check_version(link.hello(), PUT_NEEDS)?;
        let shown = there(path)?;
        link.stay_in_open_project();
        // Only in the song's own project, and only where the link keeps every step to it (`onlyIn`): the bars are
        // somewhere else in any other.
        if !link.only_in().is_some_and(|open| same_project(open, &bars.project)) {
            return Err(Error::NotTheSong { open: link.hello().project.clone(), song: bars.project.clone() });
        }
        let track = link.selected_track()?.ok_or(Error::NoTrackSelected)?;
        if !matches!(track.kind.as_str(), "Instrument" | "Hybrid") {
            return Err(Error::NotAnInstrumentTrack { name: track.name, kind: track.kind });
        }
        let track_id = track.id.ok_or_else(|| {
            Error::Bitwig(
                "Bitwig didn't say the selected track's ID, so the app couldn't record the part on it. Install the \
                 extension again from Settings > DAWs."
                    .into(),
            )
        })?;
        stopped(&mut link)?;
        // Bitwig's own tempo is what the recording takes its time at; a link that doesn't say gets a long wait.
        let tempo = link.tempo().ok().flatten();
        let slot = link.add_clip_to_record(&track_id, &shown)?;
        if !wanted() {
            return Err(match link.delete_audition_clip(&track_id, slot, false) {
                Ok(_) => Error::Cancelled,
                Err(e) => left_in_launcher(&e, &track.name),
            });
        }
        // The job's own waits (for Bitwig to show each setting, to start, to stop and to show everything back) get
        // what any change gets; the part's playing comes on top.
        let wait = link.change_timeout.saturating_add(playing_time(bars.length, tempo));
        let recorded = match link.record_clip(&track_id, slot, bars.start, bars.length, wait) {
            Ok(recorded) => recorded,
            // The link refused before it began, in its own words (Bitwig plays, a launcher clip plays, the bars
            // aren't whole bars of its time signature): nothing changed but the clip, which comes out again.
            Err(e @ Error::Bitwig(_)) => {
                return Err(match link.delete_audition_clip(&track_id, slot, false) {
                    Ok(_) => e,
                    Err(_) => left_in_launcher(&e, &track.name),
                });
            }
            // No answer (it ran out of time, or the connection went): the recording may still be under way, and the
            // job takes its clip out itself when it gets to the end.
            Err(e) => {
                return Err(Error::Bitwig(format!(
                    "{}. Bitwig may still be recording the part: give it a moment, then have a look there. If its \
                     clip stays in \u{201c}{}\u{201d}'s clip launcher, delete it there.",
                    sentence(&e),
                    track.name
                )));
            }
        };
        // Also has the link forget the clip was the app's, whoever took it out.
        let cleared = link.delete_audition_clip(&track_id, slot, false);
        let clip_left = !recorded.launcher_clip_deleted && cleared.is_err();
        let not_back = recorded.not_back.clone().unwrap_or_default();
        match went(&recorded) {
            Ok(seen) => Ok(PutIn { track: track.name, seen, not_back, clip_left }),
            Err(mut why) => {
                if !not_back.is_empty() {
                    why.push_str(&format!(" Bitwig didn't show these back as they were: {}.", not_back.join(", ")));
                }
                if clip_left {
                    why.push_str(&format!(
                        " The part's clip may still be in \u{201c}{}\u{201d}'s clip launcher there.",
                        track.name
                    ));
                }
                Err(Error::Bitwig(why))
            }
        }
    }
}

/// Waits for Bitwig to show it's stopped: a part the app stopped there a moment ago may still show playing. The
/// record job refuses while Bitwig plays, so one that plays on is said here, before anything goes in.
fn stopped(link: &mut Link) -> Result<(), Error> {
    let asked = Instant::now();
    while link.is_playing()? {
        if asked.elapsed() >= STOP_WAIT {
            return Err(Error::Bitwig("Bitwig is playing. Stop it, then try again.".into()));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

/// How long the record job lets the clip play at most: its length and two bars more, at Bitwig's tempo.
fn playing_time(length: f64, tempo: Option<f64>) -> Duration {
    // The link takes tempos from 20 to 666 BPM, as Bitwig does.
    let bpm = tempo.filter(|bpm| bpm.is_finite()).unwrap_or(SLOW_TEMPO).clamp(20.0, 666.0);
    // Two bars of up to 8 beats each: a bar is rarely longer.
    let seconds = (length + 16.0) * 60.0 / bpm;
    // Never past an hour, whatever comes in (and never a wait the clock can't count).
    let seconds = if seconds.is_finite() { seconds.clamp(0.0, 3600.0) } else { 3600.0 };
    Duration::from_secs_f64(seconds)
}

/// What a failure says when the part's clip couldn't be taken out of the clip launcher again.
fn left_in_launcher(why: &Error, track: &str) -> Error {
    Error::Bitwig(format!(
        "{}. The part's clip may still be in \u{201c}{track}\u{201d}'s clip launcher there: delete it in Bitwig.",
        sentence(why)
    ))
}

/// What the record job's answer comes to: how much of the part the link saw go in, or why it didn't, in words.
fn went(recorded: &Recorded) -> Result<Seen, String> {
    let why = recorded.why.trim().trim_end_matches('.');
    match recorded.decided.as_str() {
        // "recorded" is the job having heard it back from the arranger (the app doesn't ask it to: it takes as long
        // again); "notChecked" is what it says otherwise.
        "recorded" | "notChecked" => Ok(match recorded.ended.as_str() {
            "done" | "cut" if recorded.clip_played || recorded.decided == "recorded" => Seen::Whole,
            "timedOut" | "stoppedEarly" => Seen::Short,
            _ => Seen::Unseen,
        }),
        "cantTell" => Ok(Seen::Unseen),
        "nothing" => Err("Bitwig played the part, but nothing of it was in the arranger afterwards.".to_owned()),
        "refused" if recorded.refused.is_empty() => {
            Err("Bitwig didn't let the recording start, so nothing was recorded.".to_owned())
        }
        "refused" => Err(format!(
            "Bitwig didn't take what the recording needs ({}), so nothing was recorded.",
            recorded.refused.join(", ")
        )),
        "didntStart" => Err("Bitwig didn't start playing, so nothing was recorded.".to_owned()),
        "aborted" => Err(format!(
            "The recording was stopped part way{}. Some of the part may be in the arranger: Bitwig's undo takes it \
             out.",
            if why.is_empty() { String::new() } else { format!(" ({why})") }
        )),
        "abandoned" => Err("Another project came to the front in Bitwig while the part was being recorded, so the \
                            link stopped and touched nothing more. Back in your song, have a look at what it had \
                            set for the recording: the master's mute, the play start, and the transport's switches \
                            (record, loop, metronome)."
            .to_owned()),
        "failed" if why.is_empty() => Err("The recording failed in Bitwig.".to_owned()),
        "failed" => Err(format!("The recording failed in Bitwig: {why}.")),
        other => Err(format!(
            "Bitwig's answer wasn't what the app expected ({other}). Have a look at the track there; installing the \
             extension again from Settings > DAWs may help."
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(decided: &str, ended: &str, clip_played: bool) -> Recorded {
        Recorded { decided: decided.into(), ended: ended.into(), clip_played, ..Recorded::default() }
    }

    #[test]
    fn says_how_much_of_the_part_the_link_saw_go_in() {
        assert_eq!(went(&job("notChecked", "done", true)), Ok(Seen::Whole));
        // Bitwig once ignored the stop at the clip's end: the link ends the take just past it.
        assert_eq!(went(&job("notChecked", "cut", true)), Ok(Seen::Whole));
        assert_eq!(went(&job("notChecked", "timedOut", true)), Ok(Seen::Short));
        assert_eq!(went(&job("notChecked", "stoppedEarly", true)), Ok(Seen::Short));
        assert_eq!(went(&job("notChecked", "done", false)), Ok(Seen::Unseen), "the clip was never seen playing");
        // An older extension plays it back whatever it's asked: it heard the part in the arranger.
        assert_eq!(went(&job("recorded", "done", false)), Ok(Seen::Whole));
        assert_eq!(went(&job("cantTell", "done", true)), Ok(Seen::Unseen));
    }

    #[test]
    fn says_why_nothing_went_in() {
        let mut refused = job("refused", "", false);
        refused.refused = vec!["pre-roll".into(), "arm".into()];
        assert_eq!(
            went(&refused).unwrap_err(),
            "Bitwig didn't take what the recording needs (pre-roll, arm), so nothing was recorded."
        );
        assert!(went(&job("refused", "", false)).unwrap_err().contains("didn't let the recording start"));
        assert!(went(&job("didntStart", "", false)).unwrap_err().contains("didn't start playing"));
        assert!(went(&job("nothing", "done", true)).unwrap_err().contains("nothing of it was in the arranger"));
        let mut aborted = job("aborted", "aborted", true);
        aborted.why = "Changes were turned off.".into();
        let said = went(&aborted).unwrap_err();
        assert!(said.starts_with("The recording was stopped part way (Changes were turned off). Some of"), "{said}");
        assert!(went(&job("abandoned", "", false)).unwrap_err().contains("master's mute"));
        let mut failed = job("failed", "", false);
        failed.why = "The track bank wasn't made.".into();
        assert_eq!(went(&failed).unwrap_err(), "The recording failed in Bitwig: The track bank wasn't made.");
        assert!(went(&job("something new", "", false)).unwrap_err().contains("wasn't what the app expected"));
    }

    #[test]
    fn waits_as_long_as_the_part_takes_to_play() {
        // 16 beats and two long bars more at 120 BPM: 16 seconds.
        assert_eq!(playing_time(16.0, Some(120.0)), Duration::from_secs(16));
        // No tempo said: a slow one, so a recording still under way isn't given up on.
        assert_eq!(playing_time(16.0, None), Duration::from_secs(32));
        // Never past what the clock can count, whatever comes in.
        assert_eq!(playing_time(1.0e12, Some(0.0)), Duration::from_secs(3600));
        assert_eq!(playing_time(f64::NAN, Some(120.0)), Duration::from_secs(3600));
        assert_eq!(playing_time(16.0, Some(f64::NAN)), playing_time(16.0, None));
    }
}
