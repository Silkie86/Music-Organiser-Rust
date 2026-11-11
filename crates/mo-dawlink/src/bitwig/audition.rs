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
//! With the song ([`Bitwig::play_with_song`], extension 3.22): the same, with the bars of the song the part was found
//! for going round in Bitwig's arranger, and the part starting with the first of them. Stopping it stops Bitwig and
//! puts the arranger's loop and the play start back as they were (the extension does, and leaves alone any of them
//! you changed meanwhile). Only in the project the bars were read from: with another one open, or an older
//! extension, the part plays alone, and [`Playing::with_song`] says why.
//!
//! Audition (the Presets page, extension 3.20) is the same with a preset of its own: [`Bitwig::audition_preset`] puts
//! the preset on a new track and the part looping on it. [`Bitwig::stop_audition`] takes the track out again (only
//! while it keeps its name), and [`Bitwig::keep_audition`] stops it playing and leaves the track, preset and clip in
//! the project: Send to Bitwig, heard first.

use std::path::Path;

use mo_formats::paths::file_name;

use super::send::{Turn, sentence};
use super::{Bitwig, Error, Hello, Link, TakenOut};

/// The extension version Play on the selected track needs.
pub const PLAY_NEEDS: &str = "3.19";

/// The extension version Audition (a preset on a track of its own, playing a part) needs.
pub const AUDITION_NEEDS: &str = "3.20";

/// The extension version Play on the selected track needs to play with the song's bars going round ([`SongLoop`]).
pub const WITH_SONG_NEEDS: &str = "3.22";

/// The bars of a song for a part to play with in Bitwig ([`Bitwig::play_with_song`]): Bitwig's arranger goes round
/// them, and the part starts with the first.
#[derive(Clone, Debug, PartialEq)]
pub struct SongLoop {
    /// The project the bars are in, by the name Bitwig shows it under: its save's name, less ".bwproject".
    pub project: String,
    /// Where the bars start, in beats (quarter notes) from the arranger's start.
    pub start: f64,
    /// How long they are, in beats.
    pub length: f64,
}

/// Whether a part playing on the selected track has its song's bars going round with it ([`Playing::with_song`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WithSong {
    /// It wasn't asked for: the part alone, in step with wherever Bitwig is.
    NotAsked,
    /// Bitwig's arranger goes round the bars, and the part started with the first of them.
    Looping,
    /// Bitwig has another project open (its name; "" when Bitwig didn't say which): the part alone.
    OtherProject(String),
    /// What answers in Bitwig is older than [`WITH_SONG_NEEDS`] (its words: "the Music Organiser extension 3.21"):
    /// the part alone.
    OldExtension(String),
    /// This Bitwig didn't let the extension reach the arranger's loop or the play start ([`Hello::song_loop`]): the
    /// part alone.
    NoLoop,
}

/// A part playing on the selected track in Bitwig ([`Bitwig::play_on_selected`]): what stopping it needs, and the
/// track's name for the screen ("Playing on Bass in Bitwig").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Playing {
    /// The track's name, as Bitwig showed it.
    pub track: String,
    /// Whether the song's bars go round with it, and why not when they were asked for.
    pub with_song: WithSong,
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

/// Whether a failed `newtrack` may have made the track all the same: not when the app never reached it (an old link,
/// no connection), nor when Bitwig refused it in its own words, except that another project came to the front
/// ([`Error::is_elsewhere`], see [`new_track_failed`]).
fn may_have_left_a_track(e: &Error) -> bool {
    matches!(e, Error::ChangesOff | Error::TimedOut | Error::Closed | Error::Lost(_) | Error::Unexpected)
}

/// What a failed `newtrack` says. Another project in front may have come there part way, after the track was made:
/// this audition keeps to the project that was in front a moment before it asked, so the link's own "stopped before
/// changing anything" can't be taken at its word (the 10 Nov review's D-3).
fn new_track_failed(e: Error, name: &str) -> Error {
    if e.is_elsewhere() {
        Error::Bitwig(format!(
            "Another project came to the front in Bitwig while the app was adding the track, so it stopped. There may \
             be an empty track named {name} in the project you had in front before."
        ))
    } else if may_have_left_a_track(&e) {
        Error::Bitwig(format!("{}. There may be an empty track named {name} in Bitwig.", sentence(&e)))
    } else {
        e
    }
}

/// Whether Bitwig's name for the project it has open is the song's: as written, give or take capitals (a save's name
/// is the same file on Windows and Mac whichever way it's typed).
///
/// The name is all Bitwig tells the link of the project it has open (never its folder), so two saves with one name
/// can't be told apart here: with the other one open, its bars are the ones that go round. Stopping the part puts
/// its loop and play start back all the same.
fn same_project(open: &str, song: &str) -> bool {
    !open.is_empty() && open.to_lowercase() == song.to_lowercase()
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
        turn: &Turn<'_>,
        wanted: &dyn Fn() -> bool,
    ) -> Result<Playing, Error> {
        self.play_with_song(path, None, turn, wanted)
    }

    /// [`Bitwig::play_on_selected`] with the bars of the song the part was found for going round with it: Bitwig
    /// stops if it's playing, its arranger loops `song`'s bars, and the part starts with the first of them (the
    /// extension 3.22 does all three, and remembers the loop and play start it found). Stopping it stops Bitwig and
    /// puts those back.
    ///
    /// The bars are only looped in the project they were read from: when Bitwig has another open, or what answers
    /// is older than 3.22, the part plays alone, as [`Bitwig::play_on_selected`] plays it, and
    /// [`Playing::with_song`] says which. Without `song` it is that.
    pub fn play_with_song(
        &self,
        path: impl AsRef<Path>,
        song: Option<&SongLoop>,
        _turn: &Turn<'_>,
        wanted: &dyn Fn() -> bool,
    ) -> Result<Playing, Error> {
        let path = path.as_ref();
        if !wanted() {
            return Err(Error::Cancelled);
        }
        if let Some(song) = song
            && !(song.start.is_finite() && song.start >= 0.0 && song.length.is_finite() && song.length > 0.0)
        {
            return Err(Error::Invalid(format!(
                "The song's bars start at beat {} and are {} beats long, which Bitwig can't loop.",
                song.start, song.length
            )));
        }
        let mut link = self.connect()?;
        check_version(link.hello(), PLAY_NEEDS)?;
        let shown = there(path)?;
        link.stay_in_open_project();
        let with_song = match song {
            None => WithSong::NotAsked,
            Some(_) if !link.hello().at_least(WITH_SONG_NEEDS) => WithSong::OldExtension(link.hello().answering()),
            // Asking would be refused, with nothing played: the part alone still plays. Said before which project
            // is open: bringing the song to the front wouldn't help, so this is the reason that would still stand.
            Some(_) if link.hello().song_loop == Some(false) => WithSong::NoLoop,
            // Only where the link keeps the change to that project (`onlyIn`), so the bars are never looped in
            // another that came to the front meanwhile.
            Some(song) if link.only_in().is_some_and(|open| same_project(open, &song.project)) => WithSong::Looping,
            Some(_) => WithSong::OtherProject(link.hello().project.clone()),
        };
        let looped = song.filter(|_| with_song == WithSong::Looping);
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
        // If Bitwig can't say, take it as playing: stopping then leaves Bitwig as it is. With the song's bars the
        // link stops Bitwig first and starts it at the first of them, so stopping the part always stops Bitwig.
        let was_playing = looped.is_none() && link.is_playing().unwrap_or(true);
        let slot = link.add_audition_clip(&track_id, &shown, looped.map(|song| (song.start, song.length)))?;
        let playing = Playing {
            track: track.name,
            with_song,
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
    /// the part started it. [`TakenOut`] says whether the clip was taken out (not when it was renamed: it's yours
    /// now, or is gone), and, for a part that played with its song's bars, whether Bitwig showed its loop and play
    /// start back as they were.
    ///
    /// It goes in `_turn` like everything that changes the project, so it never lands between another start's steps
    /// (where stopping Bitwig would silence that one).
    pub fn stop_playing(&self, playing: &Playing, _turn: &Turn<'_>) -> Result<TakenOut, Error> {
        self.stop_playing_now(playing)
    }

    /// [`Bitwig::stop_playing`] without waiting its turn, for when the app closes: the threads that would go first
    /// are going too, and the extension (3.21) still makes the stop after the app has gone.
    pub fn stop_playing_as_app_closes(&self, playing: &Playing) -> Result<TakenOut, Error> {
        self.stop_playing_now(playing)
    }

    fn stop_playing_now(&self, playing: &Playing) -> Result<TakenOut, Error> {
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
        turn: &Turn<'_>,
        wanted: &dyn Fn() -> bool,
    ) -> Result<Auditioning, Error> {
        self.audition_preset_telling(preset, part, name, turn, wanted, &|_| {})
    }

    /// [`Bitwig::audition_preset`], telling `made` the audition as soon as its track is made, before the preset
    /// loads (which can take Bitwig a while), so an app closing meanwhile can still take the track out (the 10 Nov
    /// review's D-2).
    pub fn audition_preset_telling(
        &self,
        preset: impl AsRef<Path>,
        part: impl AsRef<Path>,
        name: &str,
        _turn: &Turn<'_>,
        wanted: &dyn Fn() -> bool,
        made: &dyn Fn(&Auditioning),
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
        // Bitwig may have made the track before the link stopped (changes turned off part way, say); a refusal
        // before anything changed keeps its own words.
        let track = link.new_audition_track(name).map_err(|e| new_track_failed(e, name))?;
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
        made(&auditioning);
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
        if let Err(e) = link.add_audition_clip(&track_id, &part, None) {
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
    fn a_new_track_stopped_by_another_project_may_be_left() {
        let elsewhere = Error::Bitwig(
            "Bitwig has \"B\" open, not \"A\", so the link stopped before changing anything there.".into(),
        );
        let said = new_track_failed(elsewhere, "Pad").to_string();
        assert!(said.contains("may be an empty track named Pad"), "the switch can come once the track is made: {said}");
        assert!(!said.contains("before changing anything"), "never says both: {said}");
        let refused = Error::Bitwig("Bitwig has no project open.".into());
        assert_eq!(new_track_failed(refused.clone(), "Pad"), refused, "its own words, no track to look for");
        assert!(may_have_left_a_track(&Error::TimedOut));
        assert!(!may_have_left_a_track(&Error::NotRunning), "it never reached Bitwig");
        assert!(!may_have_left_a_track(&Error::OldLink));
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
