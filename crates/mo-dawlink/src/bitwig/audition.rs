//! Play on the selected track: a MIDI part plays in Bitwig through whatever is on the track selected there (a preset
//! you're trying, a synth you've set up), as a looping launcher clip, until the app stops it and takes the clip out
//! again. For auditioning a part with a sound, or a sound with a part, without leaving the app.
//!
//! Needs the extension 3.19 or later (`selectedtrack`, `clip` with `audition`, `deleteclip`), and "Let the app make
//! changes" on. Like every send, it keeps to the project Bitwig had open as it began, and goes in its turn behind
//! the sends asked for before it.
//!
//! The clip is the app's only while it keeps the name it came in with: [`Bitwig::stop_playing`] takes it out then,
//! and never a clip you made or renamed (the extension checks, not the app). If Bitwig wasn't playing before, it
//! stops Bitwig too, so the song doesn't play on.
//!
//! Audition (the Presets page, extension 3.20) is the same with a preset of its own: [`Bitwig::audition_preset`] puts
//! the preset on a new track and the part looping on it. [`Bitwig::stop_audition`] takes the track out again (only
//! while it keeps its name), and [`Bitwig::keep_audition`] stops it playing and leaves the track, preset and clip in
//! the project: Send to Bitwig, heard first.

use std::path::Path;

use mo_formats::paths::file_name;

use super::send::{Turn, sentence};
use super::{Bitwig, Error, Hello, Link};

/// The extension version Play on the selected track needs.
pub const PLAY_NEEDS: &str = "3.19";

/// The extension version Audition (a preset on a track of its own, playing a part) needs.
pub const AUDITION_NEEDS: &str = "3.20";

/// A part playing on the selected track in Bitwig ([`Bitwig::play_on_selected`]): what stopping it needs, and the
/// track's name for the screen ("Playing on Bass in Bitwig").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Playing {
    /// The track's name, as Bitwig showed it.
    pub track: String,
    track_id: String,
    slot: usize,
    /// Whether the part started Bitwig playing: stopping it stops Bitwig then.
    started_bitwig: bool,
    /// The project it's in, so stopping never touches another one that came to the front.
    project: Option<String>,
}

impl Playing {
    /// The launcher slot its clip is in.
    pub fn slot(&self) -> usize {
        self.slot
    }
}

/// Refuses, with nothing sent, when the extension is older than `needs`.
fn check_version(hello: &Hello, needs: &'static str) -> Result<(), Error> {
    hello.can_send()?;
    if !hello.at_least(needs) {
        let has = if hello.runtime == "java" { hello.answering() } else { String::new() };
        return Err(Error::OldExtension { has, needs });
    }
    Ok(())
}

/// A preset playing a part on a track of its own in Bitwig ([`Bitwig::audition_preset`]): what stopping or keeping it
/// needs, and the track's name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Auditioning {
    /// The track's name, as Bitwig showed it (the preset's).
    pub track: String,
    track_id: String,
    started_bitwig: bool,
    project: Option<String>,
}

/// Whether a failed `newtrack` may have made the track all the same: not when the link refused before changing
/// anything (another project in front as it began, an old link, no connection).
fn may_have_left_a_track(e: &Error) -> bool {
    match e {
        Error::Bitwig(words) => !words.contains("before changing anything"),
        Error::ChangesOff | Error::TimedOut | Error::Closed | Error::Lost(_) | Error::Unexpected => true,
        _ => false,
    }
}

/// Checks a file the link is to load is there, as each send does.
fn there(path: &Path) -> Result<String, Error> {
    let shown = path.to_string_lossy().into_owned();
    if !path.is_file() {
        return Err(Error::Missing { file: file_name(&shown).to_owned(), path: shown });
    }
    Ok(shown)
}

impl Bitwig {
    /// Plays the MIDI file at `path` on the track selected in Bitwig, looping, through what's on it: the file goes in
    /// the track's first empty launcher slot and is launched in step with the song (which starts Bitwig if it's
    /// stopped). Bitwig has read the file once this returns, so the caller may delete it then. Fails with nothing
    /// changed when no instrument track is selected, the extension is older than 3.19, or changes are off.
    ///
    /// It goes in `_turn` ([`super::ticket`]), behind the sends, plays and stops asked for before it. `wanted` says
    /// whether it's still wanted: asked first and again once the clip is in, so one asked for again meanwhile (or
    /// stopped) stops at once, taking out its clip inside its turn ([`Error::Cancelled`]), and newest wins. When its
    /// clip can't be taken out again, it comes back all the same, for the caller to stop (and say why it can't).
    pub fn play_on_selected(
        &self,
        path: impl AsRef<Path>,
        _turn: &Turn<'_>,
        wanted: &dyn Fn() -> bool,
    ) -> Result<Playing, Error> {
        let path = path.as_ref();
        if !wanted() {
            return Err(Error::Cancelled);
        }
        let mut link = self.connect()?;
        check_version(link.hello(), PLAY_NEEDS)?;
        let shown = there(path)?;
        link.stay_in_open_project();
        let track = link.selected_track()?.ok_or(Error::NoTrackSelected)?;
        if !matches!(track.kind.as_str(), "Instrument" | "Hybrid") {
            return Err(Error::NotAnInstrumentTrack { name: track.name, kind: track.kind });
        }
        let track_id = track.id.ok_or_else(|| {
            Error::Bitwig(
                "Bitwig didn't say the selected track's ID, so the app couldn't take the part out again. Install the \
                 extension again from Settings > DAWs."
                    .into(),
            )
        })?;
        // If Bitwig can't say, take it as playing: stopping then leaves Bitwig as it is.
        let was_playing = link.is_playing().unwrap_or(true);
        let slot = link.add_audition_clip(&track_id, &shown)?;
        let playing = Playing {
            track: track.name,
            track_id,
            slot,
            started_bitwig: !was_playing,
            project: link.only_in().map(str::to_owned),
        };
        if !wanted() {
            // Left playing when it can't come out: given back, so it can still be stopped (the review's D-fp3).
            if link.delete_audition_clip(&playing.track_id, playing.slot, playing.started_bitwig).is_ok() {
                return Err(Error::Cancelled);
            }
        }
        Ok(playing)
    }

    /// Stops a part [`Bitwig::play_on_selected`] started and takes its clip out of the track; Bitwig stops too when
    /// the part started it. Whether the clip was taken out: false when it was renamed (it's yours now) or is gone.
    ///
    /// It goes in `_turn` like everything that changes the project, so it never lands between another start's steps
    /// (where stopping Bitwig would silence that one).
    pub fn stop_playing(&self, playing: &Playing, _turn: &Turn<'_>) -> Result<bool, Error> {
        self.stop_playing_now(playing)
    }

    /// [`Bitwig::stop_playing`] without waiting its turn, for when the app closes: the threads that would go first
    /// are going too, and the extension (3.21) still makes the stop after the app has gone.
    pub fn stop_playing_as_app_closes(&self, playing: &Playing) -> Result<bool, Error> {
        self.stop_playing_now(playing)
    }

    fn stop_playing_now(&self, playing: &Playing) -> Result<bool, Error> {
        let mut link: Link = self.connect()?;
        check_version(link.hello(), PLAY_NEEDS)?;
        link.set_only_in(playing.project.clone());
        link.delete_audition_clip(&playing.track_id, playing.slot, playing.started_bitwig)
    }

    /// Auditions a preset: a new track named `name` with the preset at `preset` on it, and the MIDI file at `part`
    /// looping on it in step with the song (which starts Bitwig if it's stopped). Bitwig has read both files once this
    /// returns. When the preset or the part doesn't go in, the track comes out again and the error says why. Fails
    /// with nothing changed when the extension is older than 3.20 or changes are off.
    ///
    /// Like [`Bitwig::play_on_selected`], it goes in `_turn` and asks `wanted` first and before the clip launches;
    /// one no longer wanted takes its track out inside its turn ([`Error::Cancelled`]), or comes back all the same
    /// when the track can't come out, for the caller to stop.
    pub fn audition_preset(
        &self,
        preset: impl AsRef<Path>,
        part: impl AsRef<Path>,
        name: &str,
        _turn: &Turn<'_>,
        wanted: &dyn Fn() -> bool,
    ) -> Result<Auditioning, Error> {
        if !wanted() {
            return Err(Error::Cancelled);
        }
        let mut link = self.connect()?;
        check_version(link.hello(), AUDITION_NEEDS)?;
        let preset = there(preset.as_ref())?;
        let part = there(part.as_ref())?;
        link.stay_in_open_project();
        // If Bitwig can't say, take it as playing: stopping then leaves Bitwig as it is.
        let was_playing = link.is_playing().unwrap_or(true);
        let track = link.new_audition_track(name).map_err(|e| {
            // Bitwig may have made the track before the link stopped (changes turned off part way, say); a refusal
            // before anything changed keeps its own words.
            if may_have_left_a_track(&e) {
                Error::Bitwig(format!("{}. There may be an empty track named {name} in Bitwig.", sentence(&e)))
            } else {
                e
            }
        })?;
        let Some(track_id) = track.id.clone() else {
            return Err(Error::Bitwig(format!(
                "Bitwig didn't say the new track's ID, so the app couldn't take it out again: there's an empty track \
                 named {name} in Bitwig. Install the extension again from Settings > DAWs."
            )));
        };
        let auditioning = Auditioning {
            track: track.name.clone(),
            track_id: track_id.clone(),
            started_bitwig: !was_playing,
            project: link.only_in().map(str::to_owned),
        };
        // Whatever goes wrong from here, the track comes out again, so a failed audition leaves nothing behind (and
        // Bitwig stops if its clip launched after all and started it). When the link can't take it out (the
        // connection went, say), the words say there may be a track left; one no longer wanted comes back then, so
        // it can still be stopped (the review's D-fp3).
        let undo = |link: &mut Link, why: Error| match link.delete_audition_track(&track_id, !was_playing) {
            Ok(_) => Err(why),
            Err(_) if why == Error::Cancelled => Ok(auditioning.clone()),
            Err(_) => {
                Err(Error::Bitwig(format!("{}. There may be a track named {name} left in Bitwig.", sentence(&why))))
            }
        };
        match link.insert_file_on(&track, &preset) {
            Ok(inserted) if inserted.added => {}
            Ok(_) => {
                let why = Error::Bitwig(format!("Bitwig didn't load {}.", file_name(&preset)));
                return undo(&mut link, why);
            }
            Err(e) => return undo(&mut link, e),
        }
        if !wanted() {
            return undo(&mut link, Error::Cancelled);
        }
        if let Err(e) = link.add_audition_clip(&track_id, &part) {
            return undo(&mut link, e);
        }
        if !wanted() {
            return undo(&mut link, Error::Cancelled);
        }
        Ok(auditioning)
    }

    /// Stops an audition and takes its track out of the project, only while it keeps the name it came in with; Bitwig
    /// stops too when the audition started it. Whether the track was taken out: false when it was renamed (it's yours
    /// now) or is gone. It goes in `_turn`, as [`Bitwig::stop_playing`] does.
    pub fn stop_audition(&self, auditioning: &Auditioning, _turn: &Turn<'_>) -> Result<bool, Error> {
        self.stop_audition_now(auditioning)
    }

    /// [`Bitwig::stop_audition`] without waiting its turn, for when the app closes.
    pub fn stop_audition_as_app_closes(&self, auditioning: &Auditioning) -> Result<bool, Error> {
        self.stop_audition_now(auditioning)
    }

    fn stop_audition_now(&self, auditioning: &Auditioning) -> Result<bool, Error> {
        let mut link = self.connect()?;
        check_version(link.hello(), AUDITION_NEEDS)?;
        link.set_only_in(auditioning.project.clone());
        link.delete_audition_track(&auditioning.track_id, auditioning.started_bitwig)
    }

    /// Keeps an audition: it stops playing (and Bitwig too, when the audition started it), and the track, its preset
    /// and the part's clip stay in the project as yours. It takes the audition, so nothing can stop it (and take the
    /// track out) after. It goes in `_turn`.
    pub fn keep_audition(&self, auditioning: Auditioning, _turn: &Turn<'_>) -> Result<(), Error> {
        let mut link = self.connect()?;
        check_version(link.hello(), AUDITION_NEEDS)?;
        link.set_only_in(auditioning.project.clone());
        link.stop_track(&auditioning.track_id, auditioning.started_bitwig)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hello(runtime: &str, extension: &str) -> Hello {
        Hello { runtime: runtime.into(), extension: extension.into(), ..Hello::default() }
    }

    #[test]
    fn needs_the_extension_3_19() {
        assert!(hello("java", "3.19").at_least(PLAY_NEEDS));
        assert!(hello("java", "3.20").at_least(PLAY_NEEDS));
        assert!(hello("java", "4.0").at_least(PLAY_NEEDS));
        assert!(hello("java", "3.19.1").at_least(PLAY_NEEDS));
        assert!(!hello("java", "3.18").at_least(PLAY_NEEDS));
        assert!(!hello("java", "3.2").at_least(PLAY_NEEDS));
        assert!(!hello("java", "").at_least(PLAY_NEEDS));
        assert!(!hello("", "3.19").at_least(PLAY_NEEDS));
    }
}
