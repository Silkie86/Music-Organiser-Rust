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
//!    uses): the same sound saved again with other notes in the file; else
//! 4. a sound with the same file name whose audio is exactly the same once digital silence (samples that are exactly
//!    zero) is cut from both ends: Bitwig pads a sample it records with silence before and after (his Stylus Drums
//!    snares, 12 Nov 2026: 0.34 s in his sample folder, 4 s in the multisample, the sound itself the same to the last
//!    bit). Only exact zeros are cut, so a sound trimmed or faded differently is still not taken.
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
    /// The multisample's file, to read the sample from again ([`save_missing`]).
    pub from: PathBuf,
    /// The sample's file name.
    pub sample: String,
    /// Why it wasn't found.
    pub why: Why,
}

/// Why a sample wasn't found in the library.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// No sound in the library has its name, nor its audio under another name: the sample is only inside the
    /// multisample (Bitwig recorded it from an instrument, or its file isn't in a sample folder the app scans).
    NoSuchName,
    /// A sound with its name is in the library, but its audio is different (the file was edited, trimmed or saved
    /// again after the multisample was made).
    OtherAudio,
    /// The app couldn't read the sample's audio, so it could only look for the same file by name.
    Unplayable,
}

impl Why {
    /// A few plain words, for the card's list and the log.
    pub fn words(self) -> &'static str {
        match self {
            Why::NoSuchName => "no sound by this name",
            Why::OtherAudio => "same name, different audio",
            Why::Unplayable => "its audio couldn't be read",
        }
    }
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
    /// How many of those were found only once the silence Bitwig added around them was cut (step 4).
    pub samples_unpadded: usize,
    /// Every library sound they were found as, each once, in order.
    pub sounds: Vec<i64>,
    /// The type each of those sounds' multisample names ("Snare 3 Select": Snare), for the ones whose multisample
    /// names one ([`type_of`]).
    pub types: Vec<(i64, &'static str)>,
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

/// Whether a file's whole name is one [`save_missing`] writes a sample under before giving it its name.
pub fn is_half_saved(name: &str) -> bool {
    is_temp_file(name) && name.ends_with(".part")
}

/// A sample written to Temp, removed when this goes.
struct InTemp(PathBuf);

impl Drop for InTemp {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// The library sounds that are the sample `name` holding `bytes` (see the top of this file), each once, or why there
/// are none. Uses `temp` (the system's Temp folder) only when its bytes aren't a library file's.
/// `Ok((found, true))`: found by step 4, once the silence was cut.
fn find(library: &Library, name: &str, bytes: &[u8], temp: &Path) -> Result<(Vec<usize>, bool), Why> {
    let found = find_all(library, name, bytes, temp);
    match found {
        Ok(found) if !found.0.is_empty() => Ok(found),
        Ok(_) if library.named(name).is_empty() => Err(Why::NoSuchName),
        Ok(_) => Err(Why::OtherAudio),
        Err(()) => Err(Why::Unplayable),
    }
}

/// [`find`]'s steps: `Err` when the sample's audio couldn't be read (only step 1 was tried, and found nothing).
fn find_all(library: &Library, name: &str, bytes: &[u8], temp: &Path) -> Result<(Vec<usize>, bool), ()> {
    let path_of = |i: usize| Path::new(&library.sounds[i].path).to_owned();
    // 1: the same file under its own name. Every copy of it in the library is found, not just the first.
    let named = library.named(name);
    let mut found: Vec<usize> = named.iter().copied().filter(|&i| same_bytes(&path_of(i), bytes)).collect();
    if !found.is_empty() {
        // Copies of it under other names are left: Hide duplicates keeps a favourite first.
        return Ok((found, false));
    }
    // Its length, rate and channels, from the sample itself, to find it under another name.
    let in_temp = InTemp(temp.join(temp_name(name)));
    if std::fs::write(&in_temp.0, bytes).is_err() {
        return Err(());
    }
    let Ok(head) = mo_audio::read(&in_temp.0, 0.001) else { return Err(()) };
    if head.sample_rate == 0 || head.channels.is_empty() {
        return Err(());
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
    if !found.is_empty() {
        return Ok((found, false));
    }
    // 4: the same audio under its own name once digital silence is cut from both ends, whatever the file's size.
    let small = |i: &usize| std::fs::metadata(path_of(*i)).is_ok_and(|m| m.len() <= BIGGEST_SAMPLE);
    let named: Vec<usize> = named.iter().copied().filter(small).collect();
    if !named.is_empty()
        && let Some(sample) = unpadded(&in_temp.0)
    {
        found = named.into_iter().filter(|&i| unpadded(&path_of(i)).is_some_and(|a| a == sample)).collect();
    }
    let trimmed = !found.is_empty();
    Ok((found, trimmed))
}

/// The most of a sound decoded for step 4 (a sample is 64 MB at most, about 6 minutes of 16-bit stereo).
const UNPADDED_MOST_SECONDS: f64 = 30.0 * 60.0;

/// A sound's audio with digital silence cut from both ends, for step 4: its rate, its channels, and its samples (each
/// frame's channels in turn, bit for bit). `None` when it can't be read whole and exactly, or it's all silence (never
/// taken for another).
#[derive(PartialEq)]
struct Unpadded {
    rate: u32,
    channels: usize,
    samples: Vec<u32>,
}

fn unpadded(path: &Path) -> Option<Unpadded> {
    // A 32-bit whole-number or 64-bit float WAV isn't read exactly.
    if mo_audio::can_read_from(path) && !mo_audio::reads_exactly(path) {
        return None;
    }
    let audio = mo_audio::read(path, UNPADDED_MOST_SECONDS).ok()?;
    let rate = audio.sample_rate;
    if rate == 0 || audio.channels.is_empty() || audio.seconds() + 0.01 < audio.file_seconds {
        return None;
    }
    let silent = |i: usize| audio.channels.iter().all(|c| c[i] == 0.0);
    let start = (0..audio.len()).find(|&i| !silent(i))?;
    let end = (start..audio.len()).rfind(|&i| !silent(i))? + 1;
    let samples = (start..end).flat_map(|i| audio.channels.iter().map(move |c| c[i].to_bits())).collect();
    Some(Unpadded { rate, channels: audio.channels.len(), samples })
}

/// What one multisample gave.
#[derive(Default)]
struct One {
    read: bool,
    samples: usize,
    samples_found: usize,
    samples_unpadded: usize,
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
        match find(library, &name, &bytes, temp) {
            Ok((found, unpadded)) => {
                one.samples_found += 1;
                one.samples_unpadded += usize::from(unpadded);
                one.sounds.extend(found);
            }
            Err(why) => {
                one.missing.push(Missing { multisample: m.name.clone(), from: m.path.clone(), sample: name, why });
            }
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
    let mut typed = HashSet::new();
    for (one, m) in ones.into_iter().zip(picked) {
        // The sounds found as its samples take the type its name gives, unless an earlier multisample gave them one.
        if let Some(kind) = type_of(&m.name) {
            for &i in &one.sounds {
                if typed.insert(i) {
                    found.types.push((sounds[i].id, kind));
                }
            }
        }
        found.multisamples += usize::from(one.read);
        found.samples += one.samples;
        found.samples_found += one.samples_found;
        found.samples_unpadded += one.samples_unpadded;
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
    /// How many were given the type their multisample names ([`takes_type`]).
    pub typed: usize,
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
        if marked.typed > 0 {
            words.push_str(&format!(
                " {} the type {} multisample names.",
                if marked.typed == 1 {
                    "1 was given".to_owned()
                } else {
                    format!("{} were given", thousands(marked.typed))
                },
                if marked.typed == 1 { "its" } else { "their" }
            ));
        }
        if found.samples_unpadded > 0 {
            words.push_str(&format!(
                " {} the same sound once the silence Bitwig added before and after {} was left out.",
                if found.samples_unpadded == 1 {
                    "1 was".to_owned()
                } else {
                    format!("{} were", thousands(found.samples_unpadded))
                },
                if found.samples_unpadded == 1 { "it" } else { "them" },
            ));
        }
        if !found.missing.is_empty() {
            words.push_str(&format!(" {} not found", plural(found.missing.len(), "sample")));
            let count = |why: Why| found.missing.iter().filter(|m| m.why == why).count();
            // (why, words for one, words for more): short, as the list below names each sample with its reason.
            let reasons: Vec<String> = [
                (
                    Why::NoSuchName,
                    "is only in the multisample (Bitwig may have recorded it, or its file isn't in your sample folders)",
                    "are only in the multisample (Bitwig may have recorded them, or their files aren't in your sample \
                     folders)",
                ),
                (
                    Why::OtherAudio,
                    "has a sound of yours by its name, but different audio",
                    "have a sound of yours by their name, but different audio",
                ),
                (Why::Unplayable, "couldn't be played", "couldn't be played"),
            ]
            .into_iter()
            .filter_map(|(why, one, more)| match count(why) {
                0 => None,
                1 => Some(format!("1 {one}")),
                n => Some(format!("{} {more}", thousands(n))),
            })
            .collect();
            words.push_str(&format!(": {}.", reasons.join("; ")));
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

/// The samples not found, a line for each multisample and reason: "Snare 3 Select: Hmy_Snr.wav, Hyper Snare.wav (no
/// sound by this name)". For the log.
pub fn missing_lines(found: &Found) -> Vec<String> {
    let mut lines: Vec<(String, Why, Vec<&str>)> = Vec::new();
    for m in &found.missing {
        match lines.last_mut() {
            Some((name, why, samples)) if *name == m.multisample && *why == m.why => samples.push(&m.sample),
            _ => lines.push((m.multisample.clone(), m.why, vec![&m.sample])),
        }
    }
    let mut out: Vec<String> =
        lines.into_iter().map(|(name, why, s)| format!("{name}: {} ({})", s.join(", "), why.words())).collect();
    out.extend(found.unreadable.iter().map(|(what, why)| format!("{what}: couldn't be read ({why})")));
    out
}

/// The samples not found, a line each with why ("Snare 3 Select › Hmy_Snr.wav: no sound by this name"), then the
/// files that couldn't be read: for the card's list.
pub fn missing_rows(found: &Found) -> Vec<String> {
    let rows = found.missing.iter().map(|m| format!("{} › {}: {}", m.multisample, m.sample, m.why.words()));
    rows.chain(found.unreadable.iter().map(|(what, why)| format!("{what}: couldn't be read ({why})"))).collect()
}

/// The type of sound a multisample's name gives ("Snare 3 Select": Snare, "Shakers 1 Select": Percussion), by the
/// same rules a sample's own name is read by ([`mo_sounds::types::from_path`]); `None` when it names none ("Garage").
/// Solomon, 12 Nov 2026: "you can also get information about what type of sound they are because the multisample
/// they appear in".
pub fn type_of(multisample: &str) -> Option<&'static str> {
    mo_sounds::types::from_path(multisample)
}

/// Whether a sound shown as `sound_type` (from `source`: "you", "name", "learned", "model" or "") takes the type
/// `given` by its multisample: not when you gave it a type yourself, nor when its own name gives one (a "Rim 07" in
/// "Snare 3 Select" stays as its name says), nor when it's that type already. A type the app only guessed (learned
/// from your library, or from how it sounds), or none, gives way to the multisample's.
pub fn takes_type(given: &str, sound_type: &str, source: &str) -> bool {
    !matches!(source, "you" | "name") && !sound_type.eq_ignore_ascii_case(given)
}

/// The folder in a sample folder that samples not found in the library are saved into, a folder for each
/// multisample (Solomon, 12 Nov 2026: "Yes, into a From Bitwig folder").
pub const FROM_BITWIG: &str = "From Bitwig";

/// What [`save_missing`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Saved {
    /// Every sample's file in the From Bitwig folder, written now or there from before: the sounds to make favourites
    /// once the folder is scanned.
    pub paths: Vec<PathBuf>,
    /// The type each one's multisample names, in the same order ([`type_of`]).
    pub types: Vec<Option<&'static str>>,
    /// How many were written now.
    pub written: usize,
    /// How many were there already (saved by an earlier run, byte for byte).
    pub there: usize,
    /// Samples that couldn't be saved, and why.
    pub failed: Vec<(String, String)>,
    /// Stopped before the end.
    pub stopped: bool,
}

/// A name that's safe as one file or folder name on Windows: no slashes, no `<>:"|?*`, no control characters, no dots
/// or spaces at the end, never "." or "..".
pub fn safe_name(name: &str) -> String {
    let mut out: String =
        name.chars()
            .map(|c| {
                if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
                    '_'
                } else {
                    c
                }
            })
            .collect();
    while out.ends_with(['.', ' ']) {
        out.pop();
    }
    if out.trim().is_empty() { "_".to_owned() } else { out }
}

/// Where a sample saved from `multisample` goes in the sample folder `into`.
pub fn saved_folder(into: &Path, multisample: &str) -> PathBuf {
    into.join(FROM_BITWIG).join(safe_name(multisample))
}

/// Saves the samples [`look`] couldn't find (all but those whose audio couldn't be read: a scan couldn't read those
/// either) into `into` (a sample folder) › From Bitwig › the multisample's name, so a scan makes them sounds. Never
/// writes over a file: one already there with the same bytes is taken as saved (an earlier run), one with other bytes
/// gets " (2)" and so on. Each is written under a name of the app's own first ([`temp_name`], ".part"), checked, then
/// given its name, so a half-written sample never looks like a sound (the start-up sweep removes one a crash left).
/// Multisamples are only read. Off the window's thread.
pub fn save_missing(missing: &[Missing], into: &Path, stop: &AtomicBool) -> Saved {
    let mut saved = Saved::default();
    let mut by_file: Vec<(&Path, &str, Vec<&str>)> = Vec::new();
    for m in missing.iter().filter(|m| m.why != Why::Unplayable) {
        match by_file.last_mut() {
            Some((from, _, samples)) if *from == m.from.as_path() => samples.push(&m.sample),
            _ => by_file.push((&m.from, &m.multisample, vec![&m.sample])),
        }
    }
    for (from, multisample, samples) in by_file {
        if stop.load(Ordering::Relaxed) {
            saved.stopped = true;
            break;
        }
        let fail =
            |saved: &mut Saved, sample: &str, why: String| saved.failed.push((format!("{multisample}: {sample}"), why));
        let zip = File::open(from)
            .map_err(|e| e.to_string())
            .and_then(|f| zip::ZipArchive::new(BufReader::new(f)).map_err(|e| e.to_string()));
        let mut zip = match zip {
            Ok(zip) => zip,
            Err(why) => {
                samples.iter().for_each(|s| fail(&mut saved, s, why.clone()));
                continue;
            }
        };
        let folder = saved_folder(into, multisample);
        if let Err(e) = std::fs::create_dir_all(&folder) {
            samples.iter().for_each(|s| fail(&mut saved, s, e.to_string()));
            continue;
        }
        // The zip's entries by their file names, read once.
        let mut at_of: HashMap<String, usize> = HashMap::new();
        for at in 0..zip.len() {
            if let Ok(entry) = zip.by_index(at)
                && let Ok(full) = entry.name()
                && let Some(name) = file_name(&full)
            {
                at_of.entry(name.to_owned()).or_insert(at);
            }
        }
        for sample in samples {
            let Some(&at) = at_of.get(sample) else {
                fail(&mut saved, sample, "it's no longer in the multisample".to_owned());
                continue;
            };
            let bytes = zip.by_index(at).map_err(|e| e.to_string()).and_then(|entry| {
                let mut bytes = Vec::new();
                entry.take(BIGGEST_SAMPLE + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
                if bytes.len() as u64 > BIGGEST_SAMPLE { Err("it's too big".to_owned()) } else { Ok(bytes) }
            });
            match bytes.and_then(|bytes| save_one(&folder, sample, &bytes)) {
                Ok((path, written)) => {
                    if written {
                        saved.written += 1;
                    } else {
                        saved.there += 1;
                    }
                    saved.paths.push(path);
                    saved.types.push(type_of(multisample));
                }
                Err(why) => fail(&mut saved, sample, why),
            }
        }
    }
    saved
}

/// Writes `bytes` as `sample` in `folder`, never over another file: (its path, whether it was written now, rather
/// than found there with these very bytes).
fn save_one(folder: &Path, sample: &str, bytes: &[u8]) -> Result<(PathBuf, bool), String> {
    let name = safe_name(sample);
    let (stem, kind) = match name.rsplit_once('.') {
        Some((stem, kind)) if !stem.is_empty() => (stem.to_owned(), format!(".{kind}")),
        _ => (name.clone(), String::new()),
    };
    let mut target = None;
    for n in 1..1000 {
        let path = if n == 1 { folder.join(&name) } else { folder.join(format!("{stem} ({n}){kind}")) };
        match std::fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                target = Some(path);
                break;
            }
            Err(e) => return Err(e.to_string()),
            Ok(_) if same_bytes(&path, bytes) => return Ok((path, false)),
            Ok(_) => {}
        }
    }
    let Some(target) = target else { return Err("there are too many files of its name".to_owned()) };
    let part = folder.join(temp_name("sample.part"));
    debug_assert!(is_half_saved(&part.file_name().unwrap_or_default().to_string_lossy()));
    let written = (|| -> std::io::Result<()> {
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&part)?;
        std::io::Write::write_all(&mut f, bytes)?;
        f.sync_all()
    })();
    let checked = written.map_err(|e| e.to_string()).and_then(|()| {
        if same_bytes(&part, bytes) { Ok(()) } else { Err("the copy didn't read back the same".to_owned()) }
    });
    // Given its name only when nothing took it meanwhile (a rename would write over it on some systems).
    let named = checked.and_then(|()| {
        if std::fs::symlink_metadata(&target).is_ok() {
            return Err("another file took its name meanwhile".to_owned());
        }
        std::fs::rename(&part, &target).map_err(|e| e.to_string())
    });
    match named {
        Ok(()) => Ok((target, true)),
        Err(why) => {
            let _ = std::fs::remove_file(&part);
            Err(why)
        }
    }
}

/// What saving said, for the card and the log: "Saved 966 samples into I:\Samples\From Bitwig: they become
/// favourites once the folder is scanned."
pub fn saved_summary(saved: &Saved, into: &Path) -> String {
    let folder = into.join(FROM_BITWIG);
    let mut words = if saved.written + saved.there == 0 {
        "No samples were saved.".to_owned()
    } else {
        let mut words = format!("Saved {} into {}", plural(saved.written + saved.there, "sample"), folder.display());
        if saved.there > 0 {
            words.push_str(&format!(" ({} were there from before)", thousands(saved.there)));
        }
        words.push('.');
        words
    };
    if !saved.failed.is_empty() {
        words.push_str(&format!(" {} couldn't be saved.", plural(saved.failed.len(), "sample")));
    }
    if saved.stopped {
        words.push_str(" Stopped before the end.");
    }
    words
}

/// The sound that is each of the files `paths` (`None`: not in the library yet), by path (letter case and either slash
/// aside, as Windows finds files).
pub fn sounds_at(paths: &[PathBuf], sounds: &[Sound]) -> Vec<Option<i64>> {
    let key = |p: &str| {
        let mut out = String::with_capacity(p.len());
        for c in p.chars().flat_map(char::to_lowercase) {
            let c = if c == '\\' { '/' } else { c };
            if !(c == '/' && out.ends_with('/')) {
                out.push(c);
            }
        }
        out
    };
    let mut at: HashMap<String, Option<i64>> = paths.iter().map(|p| (key(&p.to_string_lossy()), None)).collect();
    for s in sounds {
        if let Some(id) = at.get_mut(&key(&s.path)) {
            id.get_or_insert(s.id);
        }
    }
    paths.iter().map(|p| at.get(&key(&p.to_string_lossy())).copied().flatten()).collect()
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
            summary(&found, Marked { newly: 3, already: 1, typed: 0 }),
            "Found 3 of 4 samples in your sound library: 4 sounds now favourites (1 already were)."
        );
    }

    #[test]
    fn a_multisample_names_its_sounds_type_unless_you_or_their_name_did() {
        assert_eq!(type_of("Snare 3 Select"), Some("Snare"));
        assert_eq!(type_of("Kicks 1 Select"), Some("Kick"));
        assert_eq!(type_of("Shakers 1 Select"), Some("Percussion"));
        assert_eq!(type_of("Garage"), None);
        assert!(takes_type("Snare", "Unknown", ""));
        assert!(takes_type("Snare", "Tom", "model"));
        assert!(takes_type("Snare", "Tom", "learned"));
        assert!(!takes_type("Snare", "Percussion", "you"));
        assert!(!takes_type("Snare", "Percussion", "name"));
        assert!(!takes_type("Snare", "snare", "model"));
        let found = Found { samples: 2, samples_found: 2, sounds: vec![1, 2], ..Found::default() };
        let said = summary(&found, Marked { newly: 2, already: 0, typed: 2 });
        assert!(said.contains(" 2 were given the type their multisample names."), "{said}");
    }

    #[test]
    fn saved_names_are_safe_on_windows() {
        assert_eq!(safe_name("Snare: 2?"), "Snare_ 2_");
        assert_eq!(safe_name("a/b\\c"), "a_b_c");
        assert_eq!(safe_name("Kick. "), "Kick");
        assert_eq!(safe_name(".."), "_");
        assert!(is_half_saved(&temp_name("x.part")));
        assert!(!is_half_saved(&temp_name("x.wav")));
    }

    #[test]
    fn file_names_by_either_slash() {
        assert_eq!(file_name(r"I:\Samples\Snare.wav"), Some("Snare.wav"));
        assert_eq!(file_name("/x/y/Kick.wav"), Some("Kick.wav"));
        assert_eq!(file_name("/x/"), None);
    }

    #[test]
    fn missing_samples_a_line_a_multisample() {
        let miss =
            |m: &str, s: &str, why| Missing { multisample: m.into(), from: PathBuf::new(), sample: s.into(), why };
        let found = Found {
            samples: 5,
            samples_found: 1,
            missing: vec![
                miss("Snare 3", "a.wav", Why::NoSuchName),
                miss("Snare 3", "b.wav", Why::NoSuchName),
                miss("Snare 3", "d.wav", Why::OtherAudio),
                miss("rim 1", "c.wav", Why::NoSuchName),
            ],
            ..Found::default()
        };
        assert_eq!(
            missing_lines(&found),
            vec![
                "Snare 3: a.wav, b.wav (no sound by this name)",
                "Snare 3: d.wav (same name, different audio)",
                "rim 1: c.wav (no sound by this name)"
            ]
        );
        assert_eq!(missing_rows(&found)[2], "Snare 3 › d.wav: same name, different audio");
        let said = summary(&found, Marked { newly: 1, already: 0, typed: 0 });
        assert!(said.contains(" 4 samples not found: 3 are only in the multisample (Bitwig may"), "{said}");
        assert!(said.ends_with("; 1 has a sound of yours by its name, but different audio."), "{said}");
        assert!(!said.contains("played"), "{said}");
    }
}
