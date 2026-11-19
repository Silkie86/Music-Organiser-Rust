//! A sound tried in the Sampler selected in Bitwig, with its settings kept: the link reads the Sampler's parameters,
//! puts the sound in its place (Bitwig makes a new Sampler with it) and sets the parameters it had again.
//!
//! Bitwig's controller API can't change a Sampler's sample, but it can put a file in a device's place, which for a
//! sample makes a new Sampler. What the new one doesn't take over is set again from the old one's parameters, by
//! their IDs. A Sampler holds more than its parameters (its modulators and what they move, its zones), and those
//! don't come back: [`Swapped`] says what did. A first try, as Solomon asked (2026-11-13): untried in a real Bitwig.
//!
//! The extension says whether Bitwig showed the sound going in, and only swaps the Sampler that was selected when the
//! swap was asked for (review BW-1 and BW-3): a file Bitwig ignores is said as that, and a device selected meanwhile
//! is left alone.
//!
//! Needs the extension 3.29 or later (`insert` at `replace` that waits for the swap and takes the device to expect)
//! and "Let the app make changes" on. Bitwig's undo takes it back.

use std::path::Path;
use std::time::{Duration, Instant};

use super::audition::{check_version, there};
use super::link::Replaced;
use super::send::Turn;
use super::{Bitwig, DeviceParam, Error, SelectedDevice};

/// The extension version the swap needs.
pub const SWAP_NEEDS: &str = "3.29";

/// How near two parameter values are to count as the same (Bitwig's steps are finer than this).
const SAME: f64 = 1e-4;
/// How long Bitwig gets to show the values just set, before they're read back.
const SHOWN_AFTER: Duration = Duration::from_millis(300);
/// How often, and for how long, the new Sampler is read again while it shows no parameters yet.
const LOOK_EVERY: Duration = Duration::from_millis(100);
const PARAMS_WAIT: Duration = Duration::from_secs(2);
/// The trouble when the selection is no longer the Sampler the sound went in.
const MOVED: &str = "another device is selected in Bitwig now.";

/// What came over to the new Sampler ([`Bitwig::swap_into_sampler`]), by the parameters' names.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Swapped {
    /// The track it's on.
    pub track: String,
    /// The new Sampler's parameters that had the old one's values without being set.
    pub carried: Vec<String>,
    /// Those the link set back to the old one's values, which took.
    pub set_back: Vec<String>,
    /// Those the link set back, which Bitwig didn't show with the old value afterwards.
    pub didnt_take: Vec<String>,
    /// The old Sampler's parameters the new one doesn't have.
    pub gone: Vec<String>,
    /// The old Sampler's parameters whose value Bitwig didn't tell, so they couldn't be set back.
    pub unread: Vec<String>,
    /// The Sampler held a sound of this name already, and Bitwig showed no other sign of the swap: whether it went in
    /// can't be told, and every setting was set again in case.
    pub same_sound: bool,
    /// Why the settings weren't (all) set back after the sound went in, when something got in the way.
    pub trouble: Option<String>,
    /// Whether it can't be told that the sound went in this Sampler: the selection moved before Bitwig showed the
    /// swap, or the Sampler it showed holds another sample than the one sent. Nothing was set back then, and
    /// [`Swapped::trouble`] says which.
    pub unknown: bool,
}

impl Swapped {
    /// In plain words, for the log and the report: counts, then the names of what didn't come over.
    pub fn summary(&self) -> String {
        if self.unknown {
            return format!(
                "On \"{}\": it can't be told whether the sound went in: {} Look in Bitwig: if it did, the Sampler \
                 has none of its old settings, and Bitwig's undo puts back what changed.",
                self.track,
                self.trouble.as_deref().unwrap_or(MOVED)
            );
        }
        let mut said = format!(
            "On \"{}\": {} settings came over by themselves, {} were set back, {} didn't take, {} the new Sampler \
             doesn't have, {} weren't readable.",
            self.track,
            self.carried.len(),
            self.set_back.len(),
            self.didnt_take.len(),
            self.gone.len(),
            self.unread.len()
        );
        for (what, names) in
            [("Didn't take", &self.didnt_take), ("Not on the new one", &self.gone), ("Unreadable", &self.unread)]
        {
            if !names.is_empty() {
                said.push_str(&format!(" {what}: {}.", names.join(", ")));
            }
        }
        if self.same_sound {
            said.push_str(
                " The Sampler held a sound of this name already, and Bitwig showed no sign of another going in: \
                 look in Bitwig whether it did.",
            );
        }
        if let Some(trouble) = &self.trouble {
            said.push_str(&format!(
                " The settings weren't all set back: {trouble} Bitwig's undo puts back what changed."
            ));
        }
        said
    }
}

impl Bitwig {
    /// Puts the sample at `path` in the place of the Sampler selected in Bitwig, keeping its settings: see the
    /// module's words. Fails with nothing changed when no Sampler is selected, the file isn't there, the extension is
    /// older than 3.29, changes are off, another project came to the front, another device is selected by the time
    /// Bitwig gets to it, or Bitwig doesn't take the file. Once the sound is in, what goes wrong is told in
    /// [`Swapped::trouble`], not as an error; when it can't be told whether it is in, [`Swapped::unknown`] says so. Waits for Bitwig: call it off the UI thread, in its `_turn`.
    pub fn swap_into_sampler(&self, path: impl AsRef<Path>, _turn: &Turn<'_>) -> Result<Swapped, Error> {
        let mut link = self.connect()?;
        check_version(link.hello(), SWAP_NEEDS)?;
        link.stay_in_open_project();
        let shown = there(path.as_ref())?;
        let before = match link.selected_device()? {
            Some(d) if d.device.name == "Sampler" && !d.device.plugin => d,
            Some(d) => {
                return Err(Error::Invalid(format!(
                    "\"{}\" is selected in Bitwig, not a Sampler: select the Sampler to try the sound in.",
                    d.device.name
                )));
            }
            None => return Err(Error::Invalid("Select a Sampler in Bitwig to try the sound in.".into())),
        };
        // Without the sample's name the extension can't see the swap, and would answer before Bitwig had done it.
        if before.sample.is_none() {
            return Err(Error::Invalid(
                "This Bitwig doesn't tell the link which sample the selected Sampler holds, so a swap couldn't be \
                 checked: nothing was tried. The link's self-check says why, at \"cursor device's sample\"."
                    .into(),
            ));
        }
        let answer = link.replace_selected_with_file(&shown, &before)?;
        let mut swapped = Swapped {
            track: before.track.clone(),
            same_sound: answer.replaced == Replaced::CantTell,
            ..Swapped::default()
        };
        // The device the settings are for: the old one's place, with the sample Bitwig shows there now.
        let mut place = SelectedDevice { params: Vec::new(), ..before.clone() };
        match answer.replaced {
            Replaced::No => {
                let file =
                    path.as_ref().file_name().unwrap_or(path.as_ref().as_os_str()).to_string_lossy().into_owned();
                // The extension waited 5 seconds for it. Untried in a real Bitwig, so the words leave room for a
                // late one.
                return Err(Error::Bitwig(format!(
                    "Bitwig didn't take {file}: nothing changed. (If it shows up in the Sampler later, Bitwig's undo \
                     takes it back.)"
                )));
            }
            // The selection moved before Bitwig showed the swap: whether the sound went in can't be told (the 19 Nov
            // review's LK-3).
            Replaced::Moved => {
                swapped.trouble = Some(MOVED.into());
                swapped.unknown = true;
                return Ok(swapped);
            }
            // Only a Sampler showing the sound that was sent is the new one: another pad's Sampler, selected while
            // Bitwig swapped, sits at the same place on the same track and would get the old one's settings (the 19
            // Nov review's LK-7).
            Replaced::Yes if !names_file(&answer.sample, path.as_ref()) => {
                swapped.trouble = Some(format!(
                    "the Sampler selected in Bitwig now holds \u{201c}{}\u{201d}, not this sound, so nothing was \
                     set back.",
                    answer.sample
                ));
                swapped.unknown = true;
                return Ok(swapped);
            }
            Replaced::Yes => place.sample = Some(answer.sample.clone()),
            Replaced::CantTell => {}
        }
        // The selection, read again: only the Sampler in the old one's place gets the old one's settings. Its
        // parameters may take a moment to show.
        let started = Instant::now();
        let after = loop {
            match link.selected_device() {
                Ok(Some(now)) if same_device(&now, &place) => {
                    if !now.params.is_empty() || started.elapsed() >= PARAMS_WAIT {
                        break now;
                    }
                }
                Ok(_) => {
                    swapped.trouble = Some(MOVED.into());
                    return Ok(swapped);
                }
                Err(e) => {
                    swapped.trouble = Some(e.to_string());
                    return Ok(swapped);
                }
            }
            std::thread::sleep(LOOK_EVERY);
        };
        let mut set: Vec<(&DeviceParam, f64)> = Vec::new();
        for old in &before.params {
            let Some(value) = old.value else {
                swapped.unread.push(old.name.clone());
                continue;
            };
            match after.params.iter().find(|p| p.id == old.id) {
                None => swapped.gone.push(old.name.clone()),
                // Only a value Bitwig told after the swap is the new Sampler's for sure: any other may be the old
                // one's still, and is set again (which does no harm when it was right).
                Some(new) if new.value.is_some_and(|v| (v - value).abs() < SAME) && answer.fresh.contains(&old.id) => {
                    swapped.carried.push(old.name.clone())
                }
                // After a set that failed, the rest are left: the selection has most likely moved.
                Some(_) if swapped.trouble.is_some() => swapped.didnt_take.push(old.name.clone()),
                Some(_) => match link.set_parameter_on(&place, &old.id, value) {
                    Ok(()) => set.push((old, value)),
                    Err(e) => {
                        swapped.trouble = Some(e.to_string());
                        swapped.didnt_take.push(old.name.clone());
                    }
                },
            }
        }
        if !set.is_empty() {
            std::thread::sleep(SHOWN_AFTER);
            let now = match link.selected_device() {
                Ok(Some(now)) if same_device(&now, &place) => now.params,
                Ok(_) => {
                    swapped.trouble.get_or_insert(MOVED.into());
                    Vec::new()
                }
                Err(e) => {
                    swapped.trouble.get_or_insert(e.to_string());
                    Vec::new()
                }
            };
            for (old, value) in set {
                let took = now.iter().any(|p| p.id == old.id && p.value.is_some_and(|v| (v - value).abs() < SAME));
                if took { &mut swapped.set_back } else { &mut swapped.didnt_take }.push(old.name.clone());
            }
        }
        Ok(swapped)
    }
}

/// Whether `sample`, as Bitwig names the sample a Sampler holds, is the file at `path`: its name or its name without
/// the ending, whatever the case, with or without folders before it (the rule the extension has for "the same sample
/// again"; how a real Bitwig names it is untried).
fn names_file(sample: &str, path: &Path) -> bool {
    let shown = sample.rsplit(['/', '\\']).next().unwrap_or(sample).trim().to_lowercase();
    let name = |part: Option<&std::ffi::OsStr>| part.map(|n| n.to_string_lossy().to_lowercase());
    !shown.is_empty()
        && (name(path.file_name()).as_deref() == Some(&shown) || name(path.file_stem()).as_deref() == Some(&shown))
}

/// Whether `now` is the Sampler at `place`: on the same track, at the same place in its chain, with the same sample
/// (which tells apart the Samplers on a Drum Machine's pads, all first in their chain on one track).
fn same_device(now: &SelectedDevice, place: &SelectedDevice) -> bool {
    now.device.name == "Sampler"
        && !now.device.plugin
        && now.track_id == place.track_id
        && now.track == place.track
        && now.position == place.position
        && now.sample == place.sample
}
