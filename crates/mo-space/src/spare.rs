//! Spare copies (Solomon's F12 note, 25 Oct 2026: "if i have lots of aax plugins but i never use them or have a vst 2
//! version i never use show that"). The C# app's `SpareCopies`.
//!
//! Plugin installers often put the same plugin in every format: VST2, VST3, CLAP and AAX. A copy is spare when no
//! project uses that format and a format they do use stays installed; AAX copies are spare when Pro Tools, the only
//! DAW that loads them, isn't on the computer. When a project's format isn't known ("VST" from an old FL Studio save),
//! every copy of that plugin counts as used. [`remove`] sends the ones you pick to the Recycle Bin, each checked again
//! just before.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use mo_formats::case;
use mo_library::read::name_key;

use crate::admin::stamp;
use crate::clean::{Bin, Done, Problem, Refused, STOPPED, Sending, What, cant_keep};
use crate::plugin_space::SpaceKind;
use crate::plugins::{InstalledPlugin, format_of, is_waves};
use crate::size::{Stopped, is_link, measure};
use crate::{key, real_path};

/// A plugin file in a format none of your projects use, while the format they do use is installed too.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpareCopy {
    pub name: String,
    pub format: String,
    pub path: String,
    pub bytes: u64,
    /// What it held when it was found ([`look`]): it only goes while it still holds just that. `None` when it can't go
    /// (a link, on a network drive, only online in OneDrive, or some of it couldn't be read): it's listed so you see
    /// it, but [`SpareCopy::can_remove`] says no.
    pub stamp: Option<String>,
    pub why: Why,
}

/// Why a copy is spare.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Why {
    /// Only Pro Tools loads AAX, and it isn't on this computer.
    NoProTools,
    /// Your projects use these formats of it (`projects` of them do), none use this one, and `used_copies` are the
    /// files of those formats that are installed (one of them must still be there for this one to go).
    OtherFormatsUsed { used: Vec<String>, projects: u32, used_copies: Vec<String> },
}

impl SpareCopy {
    /// Whether [`remove`] may send it to the Recycle Bin (the app greys out picking one that can't go).
    pub fn can_remove(&self) -> bool {
        self.stamp.is_some()
    }

    /// What the app says, as the C# app says it.
    pub fn why_text(&self) -> String {
        match &self.why {
            Why::NoProTools => "Only Pro Tools loads AAX, and it isn't on this computer.".to_owned(),
            Why::OtherFormatsUsed { used, projects, .. } => {
                let n = if *projects == 1 { "1 project".to_owned() } else { format!("{projects} projects") };
                format!("Your projects use its {} ({n}); none use this {}.", used.join(" and "), self.format)
            }
        }
    }
}

/// The formats a project can name that say which file it loads.
const KNOWN: [&str; 4] = ["VST2", "VST3", "CLAP", "AU"];

/// Finds the spare copies among `files` (every plugin file found, AAX ones included). `used` is each plugin name your
/// projects use, with how many projects use each format; `pro_tools` says whether Pro Tools is installed; `look` says
/// how big a file or bundle is and what it holds (the app passes [`look`]; only asked for the spare ones). Biggest
/// first.
pub fn find(
    files: &[InstalledPlugin],
    used: &HashMap<String, HashMap<String, u32>>,
    pro_tools: bool,
    look: impl Fn(&str) -> (u64, Option<String>),
) -> Vec<SpareCopy> {
    // Each used name by its name key, as the Plugins page groups plugins: "Serum_x64" and "Diva (x64)" are Serum and
    // Diva, but Ozone 9 and Ozone 11 are two plugins (a project's Ozone 9 doesn't load Ozone 11). "FabFilter Pro-Q 3"
    // and "Pro-Q 3" are matched below ([`close`]).
    let used_keys: Vec<(String, &HashMap<String, u32>)> =
        used.iter().map(|(name, formats)| (name_key(name), formats)).filter(|(f, _)| f.chars().count() >= 2).collect();
    // The files by name key, in the order their keys first come.
    let mut groups: Vec<(String, Vec<&InstalledPlugin>)> = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    for f in files.iter().filter(|f| !is_waves(&f.name, &f.path)) {
        let fam = name_key(&f.name);
        match at.get(&fam) {
            Some(&i) => {
                if let Some((_, g)) = groups.get_mut(i) {
                    g.push(f);
                }
            }
            None => {
                at.insert(fam.clone(), groups.len());
                groups.push((fam, vec![f]));
            }
        }
    }
    let mut spare = Vec::new();
    for (fam, copies) in groups {
        if fam.chars().count() < 2 {
            continue;
        }
        // Formats by their name in capitals, so "vst3" and "VST3" are one.
        let mut formats: BTreeMap<String, u32> = BTreeMap::new();
        for (_, f) in used_keys.iter().filter(|(u, _)| close(u, &fam)) {
            for (format, n) in f.iter() {
                let entry = formats.entry(case::key(format)).or_insert(0);
                *entry = entry.saturating_add(*n);
            }
        }
        let unknown = formats.keys().any(|f| !KNOWN.contains(&f.as_str()));
        for copy in &copies {
            let format = case::key(&copy.format);
            if format == "AAX" {
                if !pro_tools {
                    let (bytes, stamp) = look(&copy.path);
                    spare.push(SpareCopy {
                        name: copy.name.clone(),
                        format: "AAX".to_owned(),
                        path: copy.path.clone(),
                        bytes,
                        stamp,
                        why: Why::NoProTools,
                    });
                }
                continue;
            }
            // Never used at all: the Never used chip has it, whole. A format no one knows: keep everything.
            if formats.is_empty() || unknown || formats.contains_key(&format) {
                continue;
            }
            // The used formats that are installed (and their files), in the order the copies come.
            let mut kept: Vec<String> = Vec::new();
            let mut used_copies = Vec::new();
            for c in copies.iter().filter(|c| formats.contains_key(&case::key(&c.format))) {
                used_copies.push(c.path.clone());
                if !kept.iter().any(|x| case::key(x) == case::key(&c.format)) {
                    kept.push(c.format.clone());
                }
            }
            // What the projects use isn't installed: this copy may be the next best thing.
            if kept.is_empty() {
                continue;
            }
            let projects = kept.iter().map(|f| formats.get(&case::key(f)).copied().unwrap_or(0)).sum();
            let (bytes, stamp) = look(&copy.path);
            spare.push(SpareCopy {
                name: copy.name.clone(),
                format: copy.format.clone(),
                path: copy.path.clone(),
                bytes,
                stamp,
                why: Why::OtherFormatsUsed { used: kept, projects, used_copies },
            });
        }
    }
    spare.sort_by_key(|s| std::cmp::Reverse(s.bytes));
    spare
}

/// How much room a plugin file or bundle takes here (a link, or a OneDrive file that's only online, takes none) and
/// what it holds now, for [`SpareCopy::stamp`] (none for one that can't go). The app passes it to [`find`].
pub fn look(path: &str, stop: &AtomicBool) -> (u64, Option<String>) {
    let Ok(measured) = measure(Path::new(path), stop) else { return (0, None) };
    let can_go = !is_link(Path::new(path))
        && real_path(path).is_some_and(|real| cant_keep(path, &real, stop).is_none() && !stop.load(Ordering::Relaxed));
    (measured.bytes, if can_go { stamp_of(path) } else { None })
}

/// Where a copy really is and what it holds (each file inside, its size and time): a folder on the way to it that
/// leads somewhere else now changes it too.
fn stamp_of(path: &str) -> Option<String> {
    let real = real_path(path)?;
    Some(format!("{}\n{}", key(&real), stamp(Path::new(path))?))
}

/// Sends the spare copies you picked to the Recycle Bin. Each is checked again just before it goes: it must still be
/// there, a plugin file or bundle of its format, not a link, not on a network drive or only online in OneDrive, and
/// hold just what it held when it was found (an update since keeps it); it must still be spare in `now` (found again
/// just now, against your projects and Pro Tools as they are now; only its paths and reasons count, so the app can find
/// it with a look that measures nothing, `|_| (0, None)`); and the copy your projects use must still be there and hold
/// something. Copies Windows guards (Program Files) all go in one administrator step at the end, so Windows asks once,
/// and each is checked again just before it asks.
pub fn remove(picked: &[SpareCopy], now: &[SpareCopy], bin: &dyn Bin, stop: &AtomicBool) -> Done {
    let mut done = Done::default();
    let now: HashMap<String, &SpareCopy> = now.iter().map(|n| (key(&n.path), n)).collect();
    let now = &now;
    let mut sending = Sending::new(bin, stop);
    // Each file once, by where it really is (worked out before any goes): the first path sent for it, by that place.
    let reals: Vec<String> = picked.iter().map(|c| real_path(&c.path).unwrap_or_else(|| c.path.clone())).collect();
    let mut seen: HashMap<String, &str> = HashMap::new();
    for (copy, real) in picked.iter().zip(reals) {
        if let Some(first) = seen.get(&key(&real)) {
            if key(first) != key(&copy.path) {
                let why = format!("It's the same file as {first}, reached another way, so it's only sent once.");
                done.problems.push(Problem { path: copy.path.clone(), why });
            }
            continue;
        }
        let copy_key = key(&copy.path);
        let check = move || -> Result<(), String> {
            if stop.load(Ordering::Relaxed) {
                return Err(STOPPED.to_owned());
            }
            still_the_copy(copy, stop)?;
            let Some(found) = now.get(&copy_key) else {
                return Err(match copy.why {
                    Why::NoProTools => {
                        "It isn't spare any more (Pro Tools may be on this computer now), so it stays.".to_owned()
                    }
                    Why::OtherFormatsUsed { .. } => format!(
                        "It isn't spare any more (a project saved since may use its {}, or the copy your projects use \
                         was removed), so it stays.",
                        copy.format
                    ),
                });
            };
            let Why::OtherFormatsUsed { used, used_copies, .. } = &found.why else { return Ok(()) };
            for used_copy in used_copies {
                if usable(used_copy, stop).map_err(|_| STOPPED.to_owned())? {
                    return Ok(());
                }
            }
            Err(format!(
                "The {} copy your projects use isn't there now (or holds nothing here: empty, or only online in \
                 OneDrive), so this one stays.",
                used.join(" and ")
            ))
        };
        if let Err(why) = check() {
            done.problems.push(Problem { path: copy.path.clone(), why });
            continue;
        }
        // Only now: one whose first path stayed is still checked by the next.
        seen.insert(key(&real), &copy.path);
        sending.send(&copy.path, real, copy.bytes, What::Plugin(SpaceKind::Program), &mut done, check);
    }
    sending.finish(&mut done);
    done
}

/// Whether a copy your projects use can still be loaded from here: it's there (a link counts when its file is) and
/// holds something on this computer (not an empty folder left behind, nor only online in OneDrive).
fn usable(path: &str, stop: &AtomicBool) -> Result<bool, Stopped> {
    let Some(real) = real_path(path) else { return Ok(false) };
    Ok(measure(Path::new(&real), stop)?.bytes > 0)
}

/// The copy is still what was found: there, a plugin file or bundle of its format, not a link, somewhere the Recycle
/// Bin can keep it, and holding just what it held then.
fn still_the_copy(copy: &SpareCopy, stop: &AtomicBool) -> Result<(), String> {
    let path = Path::new(&copy.path);
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == ErrorKind::NotFound => return Err("It isn't there any more.".to_owned()),
        Err(_) => return Err("It can't be looked at now, so it stays.".to_owned()),
    };
    if meta.file_type().is_symlink() {
        return Err(format!("{} It stays.", Refused::Link.text()));
    }
    if !format_of(&copy.path).is_some_and(|f| case::key(&f) == case::key(&copy.format)) {
        return Err(format!("It isn't a {} plugin, so it stays.", copy.format));
    }
    // A network drive first, so a bundle there isn't walked for nothing.
    let real = real_path(&copy.path).ok_or_else(|| "It isn't there any more.".to_owned())?;
    if let Some(why) = cant_keep(&copy.path, &real, stop) {
        return Err(why.to_owned());
    }
    match &copy.stamp {
        None => Err("Some of it couldn't be read when it was found, so it can't be checked now. It stays.".to_owned()),
        Some(was) if stamp_of(&copy.path).as_ref() != Some(was) => {
            Err("It changed since it was found (an update?), so it stays.".to_owned())
        }
        Some(_) => Ok(()),
    }
}

/// Whether two families are the same plugin: equal, or one with its maker in front of the other ("fabfilterproq" and
/// "proq") when both are at least four letters. C# took one anywhere inside the other, which gave "Serum FX" Serum's
/// projects.
fn close(a: &str, b: &str) -> bool {
    a == b || (a.chars().count() >= 4 && b.chars().count() >= 4 && (a.ends_with(b) || b.ends_with(a)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, format: &str) -> InstalledPlugin {
        InstalledPlugin { name: name.into(), format: format.into(), path: format!("C:/Plugins/{name}.{format}") }
    }

    fn used(list: &[(&str, &[(&str, u32)])]) -> HashMap<String, HashMap<String, u32>> {
        list.iter()
            .map(|(name, formats)| (name.to_string(), formats.iter().map(|(f, n)| (f.to_string(), *n)).collect()))
            .collect()
    }

    #[test]
    fn a_copy_in_a_format_no_project_uses_is_spare_while_the_used_one_is_installed() {
        let files = [
            file("Serum_x64", "VST2"),
            file("Serum", "VST3"),
            file("Serum", "AAX"),
            file("Vital", "CLAP"),
            file("Vital", "VST3"),
            file("Diva", "VST2"),
            file("Diva", "VST3"),
            file("Waves L2", "VST3"),
        ];
        let used = used(&[("Serum", &[("VST3", 4)]), ("Vital", &[("VST", 2)]), ("Diva(x64)", &[("vst3", 1)])]);
        let spare = find(&files, &used, false, |p| (p.len() as u64, None));
        let lines: Vec<(String, String, String)> =
            spare.iter().map(|s| (s.name.clone(), s.format.clone(), s.why_text())).collect();
        assert_eq!(
            lines,
            [
                (
                    "Serum_x64".into(),
                    "VST2".into(),
                    "Your projects use its VST3 (4 projects); none use this VST2.".into()
                ),
                // Ties keep the order the files came in.
                ("Serum".into(), "AAX".into(), "Only Pro Tools loads AAX, and it isn't on this computer.".into()),
                ("Diva".into(), "VST2".into(), "Your projects use its VST3 (1 project); none use this VST2.".into()),
            ]
        );
        // The VST3 Serum the projects use must still be there for the VST2 to go.
        let serum = spare.first().map(|s| s.why.clone());
        assert!(
            matches!(serum, Some(Why::OtherFormatsUsed { used_copies, .. }) if used_copies == ["C:/Plugins/Serum.VST3"])
        );
        // With Pro Tools, AAX copies are in use.
        assert_eq!(find(&files, &used, true, |_| (0, None)).len(), 2);
    }

    #[test]
    fn a_plugin_named_inside_another_isnt_that_one() {
        let files =
            [file("Serum FX", "VST2"), file("Serum FX", "VST3"), file("Pro-Q 3", "VST2"), file("Pro-Q 3", "VST3")];
        let used = used(&[("Serum", &[("VST3", 4)]), ("FabFilter Pro-Q 3", &[("VST3", 2)])]);
        let spare = find(&files, &used, false, |_| (0, None));
        let names: Vec<(&str, &str)> = spare.iter().map(|s| (s.name.as_str(), s.format.as_str())).collect();
        assert_eq!(names, [("Pro-Q 3", "VST2")]);
    }

    #[test]
    fn another_version_of_a_plugin_isnt_the_same_plugin() {
        // Ozone 9's VST2 was uninstalled: its projects now load the VST3, which Ozone 11's VST2 doesn't replace.
        let files = [file("Ozone 9", "VST3"), file("Ozone 11", "VST2"), file("TR-808", "VST3"), file("TR-909", "VST2")];
        let used = used(&[("Ozone 9", &[("VST2", 20)]), ("Ozone 11", &[("VST2", 3)]), ("TR-909", &[("VST2", 1)])]);
        assert!(find(&files, &used, false, |_| (0, None)).is_empty());
    }

    #[test]
    fn a_used_format_that_isnt_installed_keeps_the_other_copies() {
        let files = [file("Massive", "VST2")];
        let used = used(&[("Massive", &[("VST3", 3)])]);
        assert!(find(&files, &used, false, |_| (0, None)).is_empty());
    }
}
