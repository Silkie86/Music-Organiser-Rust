//! A sound tried in the Sampler selected in Bitwig, with its settings kept: the link reads the Sampler's parameters,
//! puts the sound in its place (Bitwig makes a new Sampler with it) and sets the parameters it had again.
//!
//! Bitwig's controller API can't change a Sampler's sample, but it can put a file in a device's place, which for a
//! sample makes a new Sampler. What the new one doesn't take over is set again from the old one's parameters, by
//! their IDs. A Sampler holds more than its parameters (its modulators and what they move, its zones), and those
//! don't come back: [`Swapped`] says what did. A first try, as Solomon asked (2026-11-13): untried in a real Bitwig.
//!
//! Needs the extension 3.25 or later (`insert` at `replace`) and "Let the app make changes" on. Bitwig's undo takes
//! it back.

use std::path::Path;
use std::time::{Duration, Instant};

use super::audition::{check_version, there};
use super::send::Turn;
use super::{Bitwig, DeviceParam, Error, Link, SelectedDevice};

/// The extension version the swap needs.
pub const SWAP_NEEDS: &str = "3.25";

/// How near two parameter values are to count as the same (Bitwig's steps are finer than this).
const SAME: f64 = 1e-4;
/// How often the link is asked whether Bitwig has made the new Sampler.
const LOOK_EVERY: Duration = Duration::from_millis(100);
/// How long a new Sampler that looks just like the old one gets before it's taken as it is.
const SETTLE: Duration = Duration::from_secs(1);

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
}

impl Swapped {
    /// In plain words, for the log and the report: counts, then the names of what didn't come over.
    pub fn summary(&self) -> String {
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
        said
    }
}

impl Bitwig {
    /// Puts the sample at `path` in the place of the Sampler selected in Bitwig, keeping its settings: see the
    /// module's words. Fails with nothing changed when no Sampler is selected, the file isn't there, the extension is
    /// older than 3.25, or changes are off. Waits for Bitwig: call it off the UI thread, in its `_turn`.
    pub fn swap_into_sampler(&self, path: impl AsRef<Path>, _turn: &Turn<'_>) -> Result<Swapped, Error> {
        let mut link = self.connect()?;
        check_version(link.hello(), SWAP_NEEDS)?;
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
        link.replace_selected_with_file(&shown)?;
        let after = made(&mut link, &before, self.change_timeout)?;
        let mut swapped = Swapped { track: after.track.clone(), ..Swapped::default() };
        let mut set: Vec<(&DeviceParam, f64)> = Vec::new();
        for old in &before.params {
            let Some(value) = old.value else {
                swapped.unread.push(old.name.clone());
                continue;
            };
            match after.params.iter().find(|p| p.id == old.id) {
                None => swapped.gone.push(old.name.clone()),
                Some(new) if new.value.is_some_and(|v| (v - value).abs() < SAME) => {
                    swapped.carried.push(old.name.clone())
                }
                Some(_) => {
                    link.set_parameter(&old.id, value)?;
                    set.push((old, value));
                }
            }
        }
        if !set.is_empty() {
            std::thread::sleep(LOOK_EVERY * 3);
            let now = link.selected_device()?.map(|d| d.params).unwrap_or_default();
            for (old, value) in set {
                let took = now.iter().any(|p| p.id == old.id && p.value.is_some_and(|v| (v - value).abs() < SAME));
                if took { &mut swapped.set_back } else { &mut swapped.didnt_take }.push(old.name.clone());
            }
        }
        Ok(swapped)
    }
}

/// The new Sampler, once Bitwig shows it selected: when its parameters differ from the old one's, or after
/// [`SETTLE`] when they don't (it may have taken them all over). An error when no Sampler comes within `wait`.
fn made(link: &mut Link, before: &SelectedDevice, wait: Duration) -> Result<SelectedDevice, Error> {
    let started = Instant::now();
    loop {
        std::thread::sleep(LOOK_EVERY);
        if let Some(now) = link.selected_device()?
            && now.device.name == "Sampler"
            && !now.params.is_empty()
            && (now.params != before.params || started.elapsed() >= SETTLE)
        {
            return Ok(now);
        }
        if started.elapsed() >= wait {
            return Err(Error::Bitwig(
                "Bitwig didn't show a Sampler with the sound in the old one's place. Its undo puts back what changed."
                    .into(),
            ));
        }
    }
}
