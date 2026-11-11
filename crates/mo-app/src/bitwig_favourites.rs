//! Favourites brought in from Bitwig multisamples (Solomon, 11 Nov 2026: "i already have some favourites but there
//! are in my bitwig multisample folder can add these to my database so i dont have to do it again"). He picked his
//! favourite drum sounds in Bitwig and saved them as multisamples ("Kicks 1 Select", "Snare 3 Select", "rim 1"): the
//! Sounds page lists the multisamples in his Bitwig Library folders, he ticks the ones that hold favourites (the ticks
//! are kept, so it can be run again), and every sound in the library that is one of their samples becomes a favourite.
//!
//! A `.multisample` file is a zip: `multisample.xml` and a copy of each sample file, under its own name, byte for byte
//! as it was (Bitwig keeps the file's own notes in it: its ACID, `smpl` and ID3 chunks). So a sample is found in the
//! library by its file, never by a name alone:
//!
//! 1. library sounds with the same file name and the very same bytes (each one: copies in other folders too); else
//! 2. one with the very same bytes under another name (a file of the same size whose analysis gives the same length,
//!    sample rate and channels); else
//! 3. one whose decoded audio is exactly the same ([`mo_sounds::copies::print_of`], the strict test Hide duplicates
//!    uses): the same sound saved again with other notes in the file.
//!
//! A sound that sounds alike but isn't the same audio is never taken. Every library sound a sample is found as
//! becomes a favourite. Nothing is written here: [`look`] only says which sounds they
//! are, and the page marks them through the one writer of your say about sounds ([`crate::sound_notes`]). The
//! multisamples are only read, never changed (Bitwig's folder is his).
//!
//! For step 3 a sample is written to the system's Temp folder for a moment, under a name of the app's own
//! ([`temp_name`]), and removed once read; the start-up sweep removes one a crash left ([`is_temp_file`], rule 28).

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufReader, Read};
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use mo_sounds::Sound;
use mo_sounds::copies::{self, Print};
use rayon::prelude::*;

use crate::songs::{plural, thousands};

/// The setting that keeps the ticked multisamples: their paths, one a line.
pub const PICKS_KEY: &str = "bitwig_favourite_multisamples";

/// The folder in a Bitwig Library its multisamples are kept in.
pub const MULTISAMPLES: &str = "Multisamples";

/// The biggest sample read out of a multisample: a bigger one, or one that says it is, is left out and said to be.
/// It's held in memory while it's looked for, and decoded whole for step 3, on each of the look's threads at once, so
/// this keeps the look's memory in bounds. Drum hits are far smaller; Bitwig's chord multisamples hold samples of a
/// few MB.
pub const BIGGEST_SAMPLE: u64 = 64 << 20;

/// How deep into a Multisamples folder multisamples are looked for (Bitwig lets you keep them in folders of your own).
const DEEPEST: usize = 8;

/// A multisample in a Bitwig Library.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Multisample {
    pub path: PathBuf,
    /// Its name, as Bitwig shows it: the file's name without ".multisample".
    pub name: String,
}

/// Whether `path` is a multisample file (by its name; ".multisample.backup" isn't one).
pub fn is_multisample(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("multisample"))
}

/// The multisamples in the Multisamples folder of each of `libraries` (Bitwig's Library folders, as Space finds them:
/// [`crate::space_bitwig::library_folders`]) and the folders in it, each once, by name (ignoring capitals), then
/// path. Links are never followed, so a folder is never listed twice or left for another drive.
pub fn multisamples(libraries: &[PathBuf]) -> Vec<Multisample> {
    let mut found: Vec<Multisample> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for library in libraries {
        let mut folders = vec![(library.join(MULTISAMPLES), 0)];
        while let Some((folder, depth)) = folders.pop() {
            let Ok(entries) = std::fs::read_dir(&folder) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                let Ok(kind) = entry.file_type() else { continue };
                if kind.is_dir() && depth < DEEPEST {
                    folders.push((path, depth + 1));
                } else if kind.is_file() && is_multisample(&path) && seen.insert(key(&path)) {
                    let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                    found.push(Multisample { path, name });
                }
            }
        }
    }
    found.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then_with(|| a.path.cmp(&b.path)));
    found
}

/// A path as a key, so the same file in other capitals (Windows) is one.
fn key(path: &Path) -> String {
    path.to_string_lossy().to_lowercase()
}

/// The ticked multisamples, as the setting keeps them ([`PICKS_KEY`]: a path a line). A tick for a multisample not
/// listed now (its drive not plugged in, say) is kept: only unticking it lets it go.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Picks(HashMap<String, PathBuf>);

impl Picks {
    /// The ticks a setting keeps.
    pub fn of(setting: &str) -> Picks {
        Picks(
            setting
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(|l| (key(Path::new(l)), PathBuf::from(l)))
                .collect(),
        )
    }

    /// The setting for these ticks.
    pub fn setting(&self) -> String {
        let mut lines: Vec<String> = self.0.values().map(|p| p.to_string_lossy().into_owned()).collect();
        lines.sort();
        lines.join("\n")
    }

    /// Whether `m` is ticked (its path in any capitals: Windows).
    pub fn has(&self, m: &Multisample) -> bool {
        self.0.contains_key(&key(&m.path))
    }

    /// Ticks `m`, or unticks it.
    pub fn set(&mut self, m: &Multisample, on: bool) {
        if on {
            self.0.insert(key(&m.path), m.path.clone());
        } else {
            self.0.remove(&key(&m.path));
        }
    }
}

/// A sample that wasn't found in the library.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Missing {
    /// The multisample's name.
    pub multisample: String,
    /// The sample's file name.
    pub sample: String,
}

/// What a [`look`] found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Found {
    /// How many multisamples were read whole.
    pub multisamples: usize,
    /// How many samples they hold.
    pub samples: usize,
    /// How many of them were found in the library.
    pub samples_found: usize,
    /// Every library sound they were found as, each once, in order.
    pub sounds: Vec<i64>,
    /// The samples not found, in the order of the multisamples.
    pub missing: Vec<Missing>,
    /// Multisamples (or samples in them) that couldn't be read, and why.
    pub unreadable: Vec<(String, String)>,
    /// The look was stopped before the end: what it found so far.
    pub stopped: bool,
}

/// What the library's sounds are known by for a look: their files' names, and their length, rate and channels.
struct Library<'a> {
    sounds: &'a [Sound],
    by_name: HashMap<String, Vec<usize>>,
    by_shape: HashMap<(i32, i32), Vec<(i64, usize)>>,
    /// Library files' prints, kept through the look: one file may be asked about by many samples.
    prints: std::sync::Mutex<HashMap<usize, Option<Print>>>,
}

/// A sound's length to the millisecond.
fn millis(seconds: f64) -> i64 {
    (seconds * 1000.0).round() as i64
}

impl<'a> Library<'a> {
    fn new(sounds: &'a [Sound]) -> Library<'a> {
        let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
        let mut by_shape: HashMap<(i32, i32), Vec<(i64, usize)>> = HashMap::new();
        for (i, s) in sounds.iter().enumerate() {
            if let Some(name) = file_name(&s.path) {
                by_name.entry(name.to_lowercase()).or_default().push(i);
            }
            if let Some(a) = &s.analysis {
                by_shape.entry((a.sample_rate, a.channel_count)).or_default().push((millis(a.seconds), i));
            }
        }
        Library { sounds, by_name, by_shape, prints: std::sync::Mutex::default() }
    }

    /// The sounds named `name` (ignoring capitals).
    fn named(&self, name: &str) -> &[usize] {
        self.by_name.get(&name.to_lowercase()).map_or(&[], Vec::as_slice)
    }

    /// The sounds of this rate and channels whose length is within a millisecond of `ms` (analyses round it).
    fn shaped(&self, rate: u32, channels: usize, ms: i64) -> Vec<usize> {
        let (Ok(rate), Ok(channels)) = (i32::try_from(rate), i32::try_from(channels)) else { return Vec::new() };
        self.by_shape
            .get(&(rate, channels))
            .map(|all| all.iter().filter(|(m, _)| (m - ms).abs() <= 1).map(|&(_, i)| i).collect())
            .unwrap_or_default()
    }

    /// The print of the sound at `i`'s file, worked out once a look (`None`: it couldn't be read).
    fn print(&self, i: usize) -> Option<Print> {
        if let Some(p) = self.prints.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(&i) {
            return *p;
        }
        let print = copies::print_of(Path::new(&self.sounds[i].path)).ok();
        self.prints.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(i, print);
        print
    }
}

/// The last part of a path, by either kind of slash (a library made on Windows read elsewhere).
fn file_name(path: &str) -> Option<&str> {
    path.rsplit(['/', '\\']).next().filter(|n| !n.is_empty())
}

/// Whether the file at `path` holds exactly `bytes`.
fn same_bytes(path: &Path, bytes: &[u8]) -> bool {
    let Ok(meta) = std::fs::metadata(path) else { return false };
    if !meta.is_file() || meta.len() != bytes.len() as u64 {
        return false;
    }
    let Ok(f) = File::open(path) else { return false };
    let mut f = BufReader::with_capacity(1 << 20, f);
    let mut buf = vec![0u8; 1 << 20];
    let mut at = 0;
    loop {
        let n = match f.read(&mut buf) {
            Ok(0) => return at == bytes.len(),
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return false,
        };
        if at + n > bytes.len() || buf[..n] != bytes[at..at + n] {
            return false;
        }
        at += n;
    }
}

/// How far a library file's size may be from a sample's to be compared by its audio (step 3): room for its notes
/// (Bitwig's samples carry ID3 and ACID chunks of a few kB) and a cover picture.
const NEAR_SIZE: u64 = 1 << 20;

/// The start of the names a sample is written to Temp under: "mo-bitwig-sample-<process id>-<count>.<its kind>".
const TEMP_PREFIX: &str = "mo-bitwig-sample-";

/// A new name for a sample in Temp, keeping its kind (".wav") so it's decoded as one.
pub fn temp_name(sample: &str) -> String {
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let n = COUNT.fetch_add(1, Ordering::Relaxed);
    let kind = Path::new(sample)
        .extension()
        .and_then(|e| e.to_str())
        .filter(|e| !e.is_empty() && e.bytes().all(|b| b.is_ascii_alphanumeric()))
        .map(str::to_ascii_lowercase)
        .unwrap_or_else(|| "bin".to_owned());
    format!("{TEMP_PREFIX}{}-{n}.{kind}", std::process::id())
}

/// Whether a file's whole name is one [`temp_name`] gives (rule 28: the start-up sweep removes only those).
pub fn is_temp_file(name: &str) -> bool {
    let Some(rest) = name.strip_prefix(TEMP_PREFIX) else { return false };
    let Some((numbers, kind)) = rest.rsplit_once('.') else { return false };
    let digits = |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit());
    !kind.is_empty()
        && kind.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && numbers.split_once('-').is_some_and(|(pid, n)| digits(pid) && digits(n))
}

/// A sample written to Temp, removed when this goes.
struct InTemp(PathBuf);

impl Drop for InTemp {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// The library sounds that are the sample `name` holding `bytes` (see the top of this file), each once. Uses `temp`
/// (the system's Temp folder) only when its bytes aren't a library file's.
fn find(library: &Library, name: &str, bytes: &[u8], temp: &Path) -> Vec<usize> {
    let path_of = |i: usize| Path::new(&library.sounds[i].path).to_owned();
    // 1: the same file under its own name. Every copy of it in the library is found, not just the first.
    let named = library.named(name);
    let mut found: Vec<usize> = named.iter().copied().filter(|&i| same_bytes(&path_of(i), bytes)).collect();
    if !found.is_empty() {
        // Copies of it under other names are left: Hide duplicates keeps a favourite first.
        return found;
    }
    // Its length, rate and channels, from the sample itself, to find it under another name.
    let in_temp = InTemp(temp.join(temp_name(name)));
    if std::fs::write(&in_temp.0, bytes).is_err() {
        return found;
    }
    let Ok(head) = mo_audio::read(&in_temp.0, 0.001) else { return found };
    if head.sample_rate == 0 || head.channels.is_empty() {
        return found;
    }
    let shaped = library.shaped(head.sample_rate, head.channels.len(), millis(head.file_seconds));
    // 2: the same bytes under another name.
    for &i in &shaped {
        if !found.contains(&i) && same_bytes(&path_of(i), bytes) {
            found.push(i);
        }
    }
    // 3: the same audio, by the strict print, among those not found yet whose file is about the sample's size (the
    // same audio with other notes in the file is within a few kB; this keeps a sample with hundreds of loops of its
    // length from decoding them all).
    let near = |i: &usize| {
        std::fs::metadata(path_of(*i)).is_ok_and(|m| (m.len() as i64 - bytes.len() as i64).unsigned_abs() <= NEAR_SIZE)
    };
    let rest: Vec<usize> = named.iter().chain(&shaped).copied().filter(|i| !found.contains(i)).filter(near).collect();
    if !rest.is_empty()
        && let Ok(print) = copies::print_of(&in_temp.0)
    {
        for i in rest {
            if !found.contains(&i) && library.print(i) == Some(print) {
                found.push(i);
            }
        }
    }
    found
}

/// What one multisample gave.
#[derive(Default)]
struct One {
    read: bool,
    samples: usize,
    samples_found: usize,
    sounds: Vec<usize>,
    missing: Vec<Missing>,
    unreadable: Vec<(String, String)>,
    stopped: bool,
}

/// Looks for the samples of the multisample `m` in the library.
fn look_in(m: &Multisample, library: &Library, temp: &Path, stop: &AtomicBool) -> One {
    let mut one = One::default();
    let mut zip = match File::open(&m.path)
        .map_err(|e| e.to_string())
        .and_then(|f| zip::ZipArchive::new(BufReader::new(f)).map_err(|e| e.to_string()))
    {
        Ok(zip) => zip,
        Err(why) => {
            one.unreadable.push((m.name.clone(), why));
            return one;
        }
    };
    for at in 0..zip.len() {
        if stop.load(Ordering::Relaxed) {
            one.stopped = true;
            return one;
        }
        let mut entry = match zip.by_index(at) {
            Ok(entry) => entry,
            Err(e) => {
                one.unreadable.push((m.name.clone(), e.to_string()));
                continue;
            }
        };
        if !entry.is_file() {
            continue;
        }
        let name = match entry.name() {
            Ok(full) => file_name(&full).map(str::to_owned),
            Err(e) => {
                one.unreadable.push((m.name.clone(), e.to_string()));
                continue;
            }
        };
        let Some(name) = name else { continue };
        // Its sample files: whatever the app can decode (multisample.xml isn't one).
        if !mo_audio::can_decode(Path::new(&name)) {
            continue;
        }
        one.samples += 1;
        if entry.size() > BIGGEST_SAMPLE {
            one.unreadable.push((format!("{}: {name}", m.name), "it's too big to look for".to_owned()));
            continue;
        }
        let mut bytes = Vec::with_capacity(usize::try_from(entry.size()).unwrap_or(0));
        // Never more than it says it is, nor than the biggest (a damaged zip can say anything).
        if let Err(e) = (&mut entry).take(BIGGEST_SAMPLE + 1).read_to_end(&mut bytes) {
            one.unreadable.push((format!("{}: {name}", m.name), e.to_string()));
            continue;
        }
        if bytes.len() as u64 > BIGGEST_SAMPLE {
            one.unreadable.push((format!("{}: {name}", m.name), "it's too big to look for".to_owned()));
            continue;
        }
        let found = find(library, &name, &bytes, temp);
        if found.is_empty() {
            one.missing.push(Missing { multisample: m.name.clone(), sample: name });
        } else {
            one.samples_found += 1;
            one.sounds.extend(found);
        }
    }
    one.read = true;
    one
}

/// Looks for the samples of `picked` in `sounds` (the library, as the Sounds page read it), the multisamples side by
/// side (rule 21; call it on a low-priority pool, never on the window's thread). `temp` is the system's Temp folder
/// (tests give their own). `done(done, of)` after each multisample. `stop` ends it early, with what was found so far.
pub fn look(
    picked: &[Multisample],
    sounds: &[Sound],
    temp: &Path,
    stop: &AtomicBool,
    done: impl Fn(usize, usize) + Sync,
) -> Found {
    let library = Library::new(sounds);
    let count = AtomicU64::new(0);
    let ones: Vec<One> = picked
        .par_iter()
        .map(|m| {
            let one = if stop.load(Ordering::Relaxed) {
                One { stopped: true, ..One::default() }
            } else {
                // A damaged file tripping a reader: that multisample is said to be unreadable, the rest go on.
                mo_catch::catch(AssertUnwindSafe(|| look_in(m, &library, temp, stop))).unwrap_or_else(|_| One {
                    unreadable: vec![(m.name.clone(), "a damaged file tripped the reader".to_owned())],
                    ..One::default()
                })
            };
            let n = count.fetch_add(1, Ordering::Relaxed) + 1;
            done(usize::try_from(n).unwrap_or(usize::MAX), picked.len());
            one
        })
        .collect();
    let mut found = Found::default();
    let mut seen = HashSet::new();
    for one in ones {
        found.multisamples += usize::from(one.read);
        found.samples += one.samples;
        found.samples_found += one.samples_found;
        found.sounds.extend(one.sounds.into_iter().filter(|&i| seen.insert(i)).map(|i| sounds[i].id));
        found.missing.extend(one.missing);
        found.unreadable.extend(one.unreadable);
        found.stopped |= one.stopped;
    }
    found
}

/// What marking the sounds found did: how many became favourites, and how many were already.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Marked {
    pub newly: usize,
    pub already: usize,
}

/// What the page says once it's marked the sounds a look found.
pub fn summary(found: &Found, marked: Marked) -> String {
    let mut words = if found.samples == 0 {
        if found.unreadable.is_empty() && !found.stopped {
            "The ticked multisamples hold no samples the app can read.".to_owned()
        } else {
            "No samples were read.".to_owned()
        }
    } else {
        let mut words = format!(
            "Found {} of {} in your sound library: {} now favourites",
            thousands(found.samples_found),
            plural(found.samples, "sample"),
            plural(marked.newly + marked.already, "sound"),
        );
        if marked.already > 0 {
            words.push_str(&format!(" ({} already were)", thousands(marked.already)));
        }
        words.push('.');
        if !found.missing.is_empty() {
            words.push_str(&format!(" {} not found.", plural(found.missing.len(), "sample")));
        }
        words
    };
    if !found.unreadable.is_empty() {
        words.push_str(&format!(" {} couldn't be read.", plural(found.unreadable.len(), "file")));
    }
    if found.stopped {
        words.push_str(" Stopped before the end.");
    }
    words
}

/// The samples not found, a line for each multisample: "Snare 3 Select: Hmy_Snr.wav, Hyper Snare.wav".
pub fn missing_lines(found: &Found) -> Vec<String> {
    let mut lines: Vec<(String, Vec<&str>)> = Vec::new();
    for m in &found.missing {
        match lines.last_mut() {
            Some((name, samples)) if *name == m.multisample => samples.push(&m.sample),
            _ => lines.push((m.multisample.clone(), vec![&m.sample])),
        }
    }
    let mut out: Vec<String> = lines.into_iter().map(|(name, s)| format!("{name}: {}", s.join(", "))).collect();
    out.extend(found.unreadable.iter().map(|(what, why)| format!("{what}: couldn't be read ({why})")));
    out
}

/// The samples not found, a line each ("Snare 3 Select › Hmy_Snr.wav"), then the files that couldn't be read: for
/// the card's list.
pub fn missing_rows(found: &Found) -> Vec<String> {
    let rows = found.missing.iter().map(|m| format!("{} › {}", m.multisample, m.sample));
    rows.chain(found.unreadable.iter().map(|(what, why)| format!("{what}: couldn't be read ({why})"))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_names_are_known_to_the_sweep() {
        let name = temp_name("HIM3-Snare-52.WAV");
        assert!(name.ends_with(".wav"), "{name}");
        assert!(is_temp_file(&name));
        assert!(is_temp_file(&temp_name("odd name")));
        for not in [
            "mo-bitwig-sample-.wav",
            "mo-bitwig-sample-1-2",
            "mo-bitwig-sample-1-x.wav",
            "my.wav",
            "mo-bitwig-sample-1-2.WAV",
        ] {
            assert!(!is_temp_file(not), "{not}");
        }
    }

    #[test]
    fn picks_keep_paths_a_line() {
        let m = |p: &str| Multisample { path: PathBuf::from(p), name: String::new() };
        let a = m(r"K:\Bitwig Studio\Library\Multisamples\Kicks 1 Select.multisample");
        let b = m(r"K:\Bitwig Studio\Library\Multisamples\rim 1.multisample");
        let mut picks = Picks::default();
        picks.set(&b, true);
        picks.set(&a, true);
        picks.set(&a, true);
        let setting = picks.setting();
        assert_eq!(setting.lines().count(), 2);
        let mut picks = Picks::of(&setting);
        assert!(picks.has(&m(r"K:\Bitwig Studio\Library\Multisamples\KICKS 1 Select.multisample")));
        assert!(!picks.has(&m(r"K:\Bitwig Studio\Library\Multisamples\Kicks 2 Select.multisample")));
        // Unticking one keeps the other, listed or not, as it was written.
        picks.set(&a, false);
        assert_eq!(picks.setting(), r"K:\Bitwig Studio\Library\Multisamples\rim 1.multisample");
    }

    #[test]
    fn summaries_say_what_happened() {
        let none = Found { stopped: true, ..Found::default() };
        assert_eq!(summary(&none, Marked::default()), "No samples were read. Stopped before the end.");
        let bad = Found { unreadable: vec![("A".into(), "damaged".into())], ..Found::default() };
        assert_eq!(summary(&bad, Marked::default()), "No samples were read. 1 file couldn't be read.");
        let found = Found { samples: 4, samples_found: 3, sounds: vec![1, 2, 3, 4], ..Found::default() };
        assert_eq!(
            summary(&found, Marked { newly: 3, already: 1 }),
            "Found 3 of 4 samples in your sound library: 4 sounds now favourites (1 already were)."
        );
    }

    #[test]
    fn file_names_by_either_slash() {
        assert_eq!(file_name(r"I:\Samples\Snare.wav"), Some("Snare.wav"));
        assert_eq!(file_name("/x/y/Kick.wav"), Some("Kick.wav"));
        assert_eq!(file_name("/x/"), None);
    }

    #[test]
    fn missing_samples_a_line_a_multisample() {
        let miss = |m: &str, s: &str| Missing { multisample: m.into(), sample: s.into() };
        let found = Found {
            missing: vec![miss("Snare 3", "a.wav"), miss("Snare 3", "b.wav"), miss("rim 1", "c.wav")],
            ..Found::default()
        };
        assert_eq!(missing_lines(&found), vec!["Snare 3: a.wav, b.wav", "rim 1: c.wav"]);
        assert_eq!(missing_rows(&found), vec!["Snare 3 › a.wav", "Snare 3 › b.wav", "rim 1 › c.wav"]);
    }
}
