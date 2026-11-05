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

use super::send::SENDS;
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

/// Whether the extension answering is `needs` ("3.19") or later; the script never is.
fn at_least(hello: &Hello, needs: &str) -> bool {
    fn parts(v: &str) -> Option<(u32, u32)> {
        let (major, minor) = v.trim().split_once('.')?;
        Some((major.parse().ok()?, minor.split('.').next()?.parse().ok()?))
    }
    hello.runtime == "java" && matches!((parts(&hello.extension), parts(needs)), (Some(has), Some(n)) if has >= n)
}

/// Refuses, with nothing sent, when the extension is older than `needs`.
fn check_version(hello: &Hello, needs: &'static str) -> Result<(), Error> {
    hello.can_send()?;
    if !at_least(hello, needs) {
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
    pub fn play_on_selected(&self, path: impl AsRef<Path>) -> Result<Playing, Error> {
        let path = path.as_ref();
        let _turn = SENDS.wait_turn();
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
                "Bitwig didn't say the selected track's ID, so the app couldn't take the part out again. Ask the \
                 extension for a selftest in Settings > DAWs to see why."
                    .into(),
            )
        })?;
        // If Bitwig can't say, take it as playing: stopping then leaves Bitwig as it is.
        let was_playing = link.is_playing().unwrap_or(true);
        let slot = link.add_audition_clip(&track_id, &shown)?;
        Ok(Playing {
            track: track.name,
            track_id,
            slot,
            started_bitwig: !was_playing,
            project: link.only_in().map(str::to_owned),
        })
    }

    /// Stops a part [`Bitwig::play_on_selected`] started and takes its clip out of the track; Bitwig stops too when
    /// the part started it. Whether the clip was taken out: false when it was renamed (it's yours now) or is gone.
    ///
    /// It doesn't wait in line behind the sends asked for before it (a kit can take seconds): it adds no track, so it
    /// can't land in the middle of one, and the link makes one change at a time anyway.
    pub fn stop_playing(&self, playing: &Playing) -> Result<bool, Error> {
        let mut link: Link = self.connect()?;
        check_version(link.hello(), PLAY_NEEDS)?;
        link.set_only_in(playing.project.clone());
        link.delete_audition_clip(&playing.track_id, playing.slot, playing.started_bitwig)
    }

    /// Auditions a preset: a new track named `name` with the preset at `preset` on it, and the MIDI file at `part`
    /// looping on it in step with the song (which starts Bitwig if it's stopped). Bitwig has read both files once this
    /// returns. When the preset or the part doesn't go in, the track comes out again and the error says why. Fails
    /// with nothing changed when the extension is older than 3.20 or changes are off.
    pub fn audition_preset(
        &self,
        preset: impl AsRef<Path>,
        part: impl AsRef<Path>,
        name: &str,
    ) -> Result<Auditioning, Error> {
        let _turn = SENDS.wait_turn();
        let mut link = self.connect()?;
        check_version(link.hello(), AUDITION_NEEDS)?;
        let preset = there(preset.as_ref())?;
        let part = there(part.as_ref())?;
        link.stay_in_open_project();
        // If Bitwig can't say, take it as playing: stopping then leaves Bitwig as it is.
        let was_playing = link.is_playing().unwrap_or(true);
        let track = link.new_audition_track(name)?;
        let Some(track_id) = track.id.clone() else {
            return Err(Error::Bitwig(
                "Bitwig didn't say the new track's ID, so the app couldn't take it out again: the track is in Bitwig \
                 with nothing on it. Ask the extension for a selftest in Settings > DAWs to see why."
                    .into(),
            ));
        };
        // Whatever goes wrong from here, the track comes out again, so a failed audition leaves nothing behind (and
        // Bitwig stops if its clip launched after all and started it).
        let undo = |link: &mut Link, why: Error| {
            let _ = link.delete_audition_track(&track_id, !was_playing);
            why
        };
        match link.insert_file(track.index, &preset) {
            Ok(inserted) if inserted.added => {}
            Ok(_) => {
                let why = Error::Bitwig(format!("Bitwig didn't load {}.", file_name(&preset)));
                return Err(undo(&mut link, why));
            }
            Err(e) => return Err(undo(&mut link, e)),
        }
        if let Err(e) = link.add_audition_clip(&track_id, &part) {
            return Err(undo(&mut link, e));
        }
        Ok(Auditioning {
            track: track.name,
            track_id,
            started_bitwig: !was_playing,
            project: link.only_in().map(str::to_owned),
        })
    }

    /// Stops an audition and takes its track out of the project, only while it keeps the name it came in with; Bitwig
    /// stops too when the audition started it. Whether the track was taken out: false when it was renamed (it's yours
    /// now) or is gone. Like [`Bitwig::stop_playing`], it doesn't wait behind the sends asked for before it.
    pub fn stop_audition(&self, auditioning: &Auditioning) -> Result<bool, Error> {
        let mut link = self.connect()?;
        check_version(link.hello(), AUDITION_NEEDS)?;
        link.set_only_in(auditioning.project.clone());
        link.delete_audition_track(&auditioning.track_id, auditioning.started_bitwig)
    }

    /// Keeps an audition: it stops playing (and Bitwig too, when the audition started it), and the track, its preset
    /// and the part's clip stay in the project as yours. It takes the audition, so nothing can stop it (and take the
    /// track out) after.
    pub fn keep_audition(&self, auditioning: Auditioning) -> Result<(), Error> {
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
        assert!(at_least(&hello("java", "3.19"), PLAY_NEEDS));
        assert!(at_least(&hello("java", "3.20"), PLAY_NEEDS));
        assert!(at_least(&hello("java", "4.0"), PLAY_NEEDS));
        assert!(at_least(&hello("java", "3.19.1"), PLAY_NEEDS));
        assert!(!at_least(&hello("java", "3.18"), PLAY_NEEDS));
        assert!(!at_least(&hello("java", "3.2"), PLAY_NEEDS));
        assert!(!at_least(&hello("java", ""), PLAY_NEEDS));
        assert!(!at_least(&hello("", "3.19"), PLAY_NEEDS));
    }
}
