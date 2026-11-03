//! `theorybench keycheck`: the key finder built in beside version 4's (the one before the rescale on Solomon's
//! packs), part by part, on MIDI files, with the key each file's name gives. It's for Solomon to run on his own packs
//! and see for himself where the two differ, before any page shows keys. It prints to the screen and writes nothing.
//!
//! Every MIDI file under the folders given (and each MIDI file given) is read as the MIDI page reads it
//! (`mo_midi::extract`), and each part that isn't drums is read by both finders, heard as the page hears it
//! (`mo_midi::keys::reading`). The key the file's name gives is found as `theorybench packs` finds it
//! ([`packs::label`]). Each part gets a line: what the old finder says, what the new one says, and "better" or
//! "worse" when only one of them gives the name's key or leans to it. The counts at the end say how often each agrees
//! with the names, the minor and major names apart.

use std::collections::HashSet;
use std::io::Write;
use std::path::{Component, MAIN_SEPARATOR_STR, Path, PathBuf};

use mo_harmony::Key;
use mo_harmony::keyfind::{self, KeyAnswer, KeyReading, Why, numbers_hash};
use mo_midi::Role;
use mo_midi::extract::{self, Found};
use rayon::prelude::*;

use crate::keyfind_v4;
use crate::packs::{self, Named};

/// What one finder said about a part: its answer, and the chance it gives that the answer is right.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Said {
    answer: KeyAnswer,
    chance: f64,
}

impl Said {
    fn of(reading: &KeyReading) -> Said {
        Said { answer: reading.answer, chance: reading.shown_chance }
    }

    /// The key it gives, or leans to when it isn't sure.
    fn key(self) -> Option<Key> {
        self.answer.leaning_key()
    }

    /// In a few words, with the chance: "C minor 91%", "A minor or C major 72%", "unsure, leaning F minor 40%".
    fn words(self) -> String {
        let chance = format!("{:.0}%", self.chance * 100.0);
        match self.answer {
            KeyAnswer::Key(k) => format!("{k} {chance}"),
            KeyAnswer::OrRelative(k) => format!("{k} or {} {chance}", k.relative()),
            KeyAnswer::HomeNote { tonic, leaning } => {
                // Spelled as the key it leans to would be: "home note F#", leaning minor.
                let key = Key { tonic, mode: leaning.unwrap_or(mo_harmony::Mode::Major) };
                let leans = match leaning {
                    Some(mo_harmony::Mode::Minor) => ", leaning minor",
                    Some(mo_harmony::Mode::Major) => ", leaning major",
                    None => "",
                };
                format!("home note {}{leans} {chance}", key.tonic_name())
            }
            KeyAnswer::Unclear { leaning: Some(k), .. } => format!("unsure, leaning {k} {chance}"),
            KeyAnswer::Unclear { why: Why::TooFewNotes, .. } => "too few notes".to_owned(),
            KeyAnswer::Unclear { why: Why::NotTonal, .. } => "not in one scale".to_owned(),
            KeyAnswer::Unclear { why: Why::Spread, .. } => "no clear key".to_owned(),
        }
    }
}

/// One part, read by both finders.
#[derive(Clone, Debug, PartialEq)]
struct Line {
    /// The part: its kind as the page guessed it, the track's name, and where it sits when the page cut the part up.
    part: String,
    /// Cut from a longer part, so heard as a window; a whole loop, heard as a loop, when not.
    cut: bool,
    old: Said,
    new: Said,
}

/// How the two finders stand on one part: "better" when only the new one gives the name's key or leans to it,
/// "worse" when only the old one does, "changed" when they say different things but neither is nearer the name.
fn mark(named: Option<Key>, old: Said, new: Said) -> &'static str {
    let agrees = |said: Said| named.is_some() && said.key() == named;
    match (agrees(old), agrees(new)) {
        (false, true) => "better",
        (true, false) => "worse",
        _ if old.answer != new.answer => "changed",
        _ => "",
    }
}

/// One file, its parts read by both finders.
struct Checked {
    /// Where it is: the name of the folder given, then the folders below it, then the file's own name.
    shown: String,
    /// What its name, or a folder's above it, says about the key.
    named: Named,
    from_folder: bool,
    /// Its parts, and how many were drums; or why it couldn't be read.
    read: Result<(Vec<Line>, usize), String>,
}

/// The folders a file sits in, from the top down: a file whose own name says nothing takes its key from the nearest
/// of them (up to three up) that names one. The folder given counts too, so a pack's own folder can label the files
/// right inside it, and a file reads the same whichever folder above it was given.
fn folders_above(path: &Path) -> Vec<String> {
    let Some(parent) = path.parent() else { return Vec::new() };
    parent
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect()
}

/// Where a file found under `root` is, for its line: the folder's name, then the way down.
fn shown(path: &Path, root: &Path) -> String {
    let mut parts: Vec<String> = root.file_name().map(|n| n.to_string_lossy().into_owned()).into_iter().collect();
    let below = path.strip_prefix(root).unwrap_or(path);
    parts.extend(below.components().filter_map(|c| match c {
        Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
        _ => None,
    }));
    parts.join(MAIN_SEPARATOR_STR)
}

/// A part as its line names it: its kind ("chords"), its track's name, and where it sits when the page cut the part
/// into pieces ("bars 9–16"). `stem` is the file's name without its ending, as the page names the clip.
fn part_name(found: &Found, stem: &str) -> String {
    let mut name = found.analysis.role.name().to_lowercase();
    let track = if found.part.is_empty() { found.track_name.as_str() } else { found.part.as_str() };
    for word in [track, found.clip.strip_prefix(stem).and_then(|r| r.strip_prefix(" · ")).unwrap_or_default()] {
        if !word.is_empty() {
            name.push_str(", ");
            name.push_str(word);
        }
    }
    name
}

/// Both finders' readings of the parts found in one file (drums aren't read), and how many parts were drums.
fn lines_of(found: &[Found], stem: &str) -> (Vec<Line>, usize) {
    mo_midi::keys::with_reader(|reader| {
        let mut drums = 0;
        let mut lines = Vec::new();
        for f in found {
            let role = f.analysis.role;
            if f.drums || role == Role::Drums {
                drums += 1;
                continue;
            }
            let old = mo_midi::keys::reading(reader, &f.notes, role, f.cut, &keyfind_v4::MODEL);
            let new = mo_midi::keys::reading(reader, &f.notes, role, f.cut, &keyfind::SHIPPED);
            if let (Some(old), Some(new)) = (old, new) {
                lines.push(Line { part: part_name(f, stem), cut: f.cut, old: Said::of(&old), new: Said::of(&new) });
            }
        }
        (lines, drums)
    })
}

/// Reads one file and its name.
fn check(path: &Path, shown: String) -> Checked {
    let text = path.to_string_lossy();
    let stem = mo_formats::paths::stem(mo_formats::paths::file_name(&text));
    let (named, from) = packs::label(stem, &folders_above(path));
    // No MIDI folder is given: a part's kind is guessed from the names of the three folders nearest it, as the MIDI
    // page guesses it below its own folder, whichever folder above them was given here.
    let read = match extract::read(path, &[]) {
        Ok(found) => Ok(lines_of(&found, stem)),
        Err(extract::Unread::InUse(why) | extract::Unread::Failed(why)) => Err(why),
    };
    Checked { shown, named, from_folder: from == "folder name", read }
}

/// How the parts in files whose names give one key came out, for one finder.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Counts {
    /// It gives the name's key, or leans to it.
    agrees: usize,
    /// It gives one key ...
    sure: usize,
    /// ... the name's.
    sure_right: usize,
    /// It gives a key or its relative ...
    pair: usize,
    /// ... and the name's key is one of the two.
    pair_right: usize,
}

impl Counts {
    fn add(&mut self, said: Said, named: Key) {
        self.agrees += usize::from(said.key() == Some(named));
        match said.answer {
            KeyAnswer::Key(k) => {
                self.sure += 1;
                self.sure_right += usize::from(k == named);
            }
            KeyAnswer::OrRelative(k) => {
                self.pair += 1;
                self.pair_right += usize::from(k == named || k.relative() == named);
            }
            KeyAnswer::HomeNote { .. } | KeyAnswer::Unclear { .. } => {}
        }
    }
}

/// The parts of one kind of name (every name, the minor ones or the major ones), for both finders.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Group {
    parts: usize,
    old: Counts,
    new: Counts,
}

impl Group {
    fn add(&mut self, line: &Line, named: Key) {
        self.parts += 1;
        self.old.add(line.old, named);
        self.new.add(line.new, named);
    }
}

/// What every file and part came to, for the counts at the end.
#[derive(Debug, Default, PartialEq)]
struct Tally {
    files: usize,
    unread: usize,
    parts: usize,
    drums: usize,
    /// Parts the two finders answer differently.
    changed: usize,
    /// Parts in files whose names give no key, and in files whose names name notes that don't settle one.
    no_key: usize,
    unclear: usize,
    /// Every name, the minor names, the major names: of every part, of the whole loops, of the pieces cut from
    /// longer parts.
    groups: [Group; 3],
    loops: [Group; 3],
    pieces: [Group; 3],
    better: usize,
    worse: usize,
}

impl Tally {
    fn add(&mut self, file: &Checked) {
        self.files += 1;
        let Ok((lines, drums)) = &file.read else {
            self.unread += 1;
            return;
        };
        self.drums += drums;
        for line in lines {
            self.parts += 1;
            self.changed += usize::from(line.old.answer != line.new.answer);
            let named = match file.named {
                Named::Key(key) => key,
                Named::Nothing => {
                    self.no_key += 1;
                    continue;
                }
                Named::Unclear => {
                    self.unclear += 1;
                    continue;
                }
            };
            match mark(Some(named), line.old, line.new) {
                "better" => self.better += 1,
                "worse" => self.worse += 1,
                _ => {}
            }
            let by_kind = if line.cut { &mut self.pieces } else { &mut self.loops };
            for g in [0, if named.is_minor() { 1 } else { 2 }] {
                for groups in [&mut self.groups, &mut *by_kind] {
                    if let Some(group) = groups.get_mut(g) {
                        group.add(line, named);
                    }
                }
            }
        }
    }
}

/// A share as a percentage, or "-" of nothing.
fn share(n: usize, of: usize) -> String {
    if of == 0 { "-".to_owned() } else { format!("{:.1}%", 100.0 * n as f64 / of as f64) }
}

/// Which lines to print.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Show {
    /// Every part.
    All,
    /// Only the parts the two finders answer differently.
    Changed,
    /// No parts, only the counts.
    Counts,
}

/// The finders' names: version 4's, and the one built in.
fn names() -> (String, String) {
    (format!("version {}", keyfind_v4::VERSION), format!("version {}", crate::keyfit::built_in_version()))
}

/// Prints each file's parts (as `show` says) and the counts.
fn report(files: &[Checked], show: Show, out: &mut impl Write) -> std::io::Result<()> {
    let (old, new) = names();
    writeln!(out, "Old: the key finder as {old} had it, before the rescale on your packs. New: {new}, built in.")?;
    if numbers_hash(&keyfind::SHIPPED) == numbers_hash(&keyfind_v4::MODEL) {
        writeln!(out, "The finder built in has {old}'s numbers, so the two say the same.")?;
    }
    if show != Show::Counts {
        writeln!(
            out,
            "\"better\": only the new one gives the key in the file's name, or leans to it. \"worse\": only the old \
             one does.\n\"changed\": they say different things, neither nearer the name.\n"
        )?;
    }
    let mut tally = Tally::default();
    for file in files {
        tally.add(file);
        if show == Show::Counts {
            continue;
        }
        let lines = match &file.read {
            Ok((lines, _)) => lines,
            Err(why) => {
                if show == Show::All {
                    writeln!(out, "{}: couldn't be read. {why}", file.shown)?;
                }
                continue;
            }
        };
        let named = match file.named {
            Named::Key(key) => Some(key),
            Named::Nothing | Named::Unclear => None,
        };
        let listed: Vec<&Line> = lines.iter().filter(|l| show == Show::All || l.old.answer != l.new.answer).collect();
        if listed.is_empty() {
            continue;
        }
        let says = match (file.named, file.from_folder) {
            (Named::Key(key), false) => format!("the name says {key}"),
            (Named::Key(key), true) => format!("a folder's name says {key}"),
            (Named::Unclear, _) => "the name doesn't settle one key".to_owned(),
            (Named::Nothing, _) => "the name gives no key".to_owned(),
        };
        writeln!(out, "{}: {says}", file.shown)?;
        for line in listed {
            let text = format!(
                "  {:<24} old {:<32} new {:<32} {}",
                line.part,
                line.old.words(),
                line.new.words(),
                mark(named, line.old, line.new)
            );
            writeln!(out, "{}", text.trim_end())?;
        }
    }
    counts(&tally, &old, &new, out)
}

/// "1 part", "2 parts".
fn plural(n: usize, one: &str) -> String {
    if n == 1 { format!("1 {one}") } else { format!("{n} {one}s") }
}

/// One line of the table: the groups it counts, and the cell of one group for one finder.
type Row = (&'static str, fn(&Tally) -> &[Group; 3], fn(&Group, &Counts) -> String);

/// The table's lines.
const ROWS: [Row; 7] = [
    ("Gives the name's key, or leans to it", |t| &t.groups, |g, c| share(c.agrees, g.parts)),
    ("  of the whole loops", |t| &t.loops, |g, c| share(c.agrees, g.parts)),
    ("  of the pieces cut from longer parts", |t| &t.pieces, |g, c| share(c.agrees, g.parts)),
    ("Sure of one key", |t| &t.groups, |g, c| share(c.sure, g.parts)),
    ("  the name's, of those", |t| &t.groups, |_, c| share(c.sure_right, c.sure)),
    ("A key or its relative", |t| &t.groups, |g, c| share(c.pair, g.parts)),
    ("  the name's one of the two, of those", |t| &t.groups, |_, c| share(c.pair_right, c.pair)),
];

/// The counts at the end.
fn counts(t: &Tally, old: &str, new: &str, out: &mut impl Write) -> std::io::Result<()> {
    writeln!(out)?;
    let drums = if t.drums > 0 { format!(", besides {}", plural(t.drums, "drum part")) } else { String::new() };
    let unread = if t.unread > 0 { format!(" {} couldn't be read.", plural(t.unread, "file")) } else { String::new() };
    writeln!(out, "{}: {} read{drums}.{unread}", plural(t.files, "MIDI file"), plural(t.parts, "part"))?;
    writeln!(out, "The two finders answer differently on {} of them ({}).", t.changed, share(t.changed, t.parts))?;
    let [every, minor, major] = t.groups;
    writeln!(
        out,
        "{} in files whose names give one key ({} minor, {} major).",
        plural(every.parts, "part"),
        minor.parts,
        major.parts
    )?;
    writeln!(
        out,
        "{} in files whose names give no key, and {} in files whose names don't settle one (chord names, two \
         keys, a lone note).",
        t.no_key, t.unclear
    )?;
    if every.parts == 0 {
        return Ok(());
    }
    writeln!(out)?;
    writeln!(out, "Of the parts whose names give one key:")?;
    writeln!(out, "{:<38}{:<18}{:<18}major names", "", "every name", "minor names")?;
    writeln!(out, "{:<38}old      new      old      new      old      new", "")?;
    for (label, groups, cell) in ROWS {
        let cells: String =
            groups(t).iter().flat_map(|g| [cell(g, &g.old), cell(g, &g.new)]).map(|c| format!("{c:<9}")).collect();
        writeln!(out, "{}", format!("{label:<38}{cells}").trim_end())?;
    }
    let parts: String = t.groups.iter().map(|g| format!("{:<18}", g.parts)).collect();
    writeln!(out, "{}", format!("{:<38}{parts}", "Parts").trim_end())?;
    writeln!(
        out,
        "\nBetter on {}: the new finder ({new}) gives the name's key, or leans to it, where the old one ({old}) \
         doesn't.\nWorse on {}: the old one does, and the new one doesn't.",
        plural(t.better, "part"),
        t.worse
    )
}

pub fn run(args: &[String]) -> Result<(), String> {
    let (mut changed, mut counts_only) = (false, false);
    let (mut folders, mut files) = (Vec::new(), Vec::new());
    for arg in args {
        match arg.as_str() {
            "--changed" => changed = true,
            "--counts" => counts_only = true,
            flag if flag.starts_with("--") => return Err(crate::USAGE.to_owned()),
            given => {
                // Windows reads \" as a quote, so a folder given as "L:\New Midi\" comes as L:\New Midi" (with a
                // quote at the end, which no name can have there).
                let given = given.strip_suffix('"').unwrap_or(given);
                let path = std::path::absolute(given).map_err(|e| format!("{given}: {e}"))?;
                if path.is_dir() {
                    folders.push(path);
                } else if path.is_file() && mo_formats::midi::is_midi_path(&path.to_string_lossy()) {
                    files.push(path);
                } else {
                    return Err(format!("{given}: not a folder or a MIDI file that can be read"));
                }
            }
        }
    }
    if folders.is_empty() && files.is_empty() {
        return Err("keycheck needs the folders the MIDI files are in, or the files".into());
    }
    let show = match (counts_only, changed) {
        (true, _) => Show::Counts,
        (false, true) => Show::Changed,
        (false, false) => Show::All,
    };
    let (found, folders_unread) = packs::midi_files(&folders)?;
    let mut seen = HashSet::new();
    let mut todo: Vec<(PathBuf, String)> = Vec::new();
    for (path, root) in found {
        let Some(root) = folders.get(root) else { continue };
        let at = shown(&path, root);
        if seen.insert(path.clone()) {
            todo.push((path, at));
        }
    }
    for path in files {
        let at = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
        if seen.insert(path.clone()) {
            todo.push((path, at));
        }
    }
    if todo.is_empty() {
        println!("No MIDI files there.");
        return Ok(());
    }
    if folders_unread > 0 {
        println!("{folders_unread} folders couldn't be opened and were passed by.");
    }
    println!("Reading {}...", plural(todo.len(), "MIDI file"));
    let checked: Vec<Checked> = todo.into_par_iter().map(|(path, at)| check(&path, at)).collect();
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    match report(&checked, show, &mut out).and_then(|()| out.flush()) {
        // Stopped part way by what it was printing to ("| more" closed): nothing's lost.
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        result => result.map_err(|e| e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mo_harmony::{Mode, PitchClass};

    fn key(name: &str) -> Key {
        Key::parse(name).unwrap()
    }

    fn said(answer: KeyAnswer, chance: f64) -> Said {
        Said { answer, chance }
    }

    #[test]
    fn each_answer_reads_in_a_few_words() {
        let f_sharp = PitchClass::new(6);
        for (answer, chance, words) in [
            (KeyAnswer::Key(key("Cm")), 0.912, "C minor 91%"),
            (KeyAnswer::OrRelative(key("Am")), 0.72, "A minor or C major 72%"),
            (
                KeyAnswer::HomeNote { tonic: f_sharp, leaning: Some(Mode::Minor) },
                0.75,
                "home note F#, leaning minor 75%",
            ),
            // Leaning to neither mode, spelled as the major key would be.
            (KeyAnswer::HomeNote { tonic: f_sharp, leaning: None }, 0.75, "home note Gb 75%"),
            (KeyAnswer::Unclear { why: Why::Spread, leaning: Some(key("Fm")) }, 0.4, "unsure, leaning F minor 40%"),
            (KeyAnswer::Unclear { why: Why::Spread, leaning: None }, 0.0, "no clear key"),
            (KeyAnswer::Unclear { why: Why::TooFewNotes, leaning: None }, 0.0, "too few notes"),
            (KeyAnswer::Unclear { why: Why::NotTonal, leaning: None }, 0.0, "not in one scale"),
        ] {
            assert_eq!(said(answer, chance).words(), words);
        }
    }

    #[test]
    fn a_part_is_better_or_worse_only_when_one_finder_alone_agrees_with_the_name() {
        let c = Some(key("C"));
        let c_major = said(KeyAnswer::Key(key("C")), 0.9);
        let a_minor_or_c = said(KeyAnswer::OrRelative(key("Am")), 0.8);
        let leaning_c = said(KeyAnswer::Unclear { why: Why::Spread, leaning: Some(key("C")) }, 0.4);
        let few = said(KeyAnswer::Unclear { why: Why::TooFewNotes, leaning: None }, 0.0);
        assert_eq!(mark(c, c_major, a_minor_or_c), "worse");
        assert_eq!(mark(c, a_minor_or_c, c_major), "better");
        // Both lean to the name's key: neither is nearer it.
        assert_eq!(mark(c, c_major, leaning_c), "changed");
        assert_eq!(mark(c, few, leaning_c), "better");
        assert_eq!(mark(c, c_major, c_major), "");
        // Without a key in the name, only whether they differ.
        assert_eq!(mark(None, c_major, a_minor_or_c), "changed");
        assert_eq!(mark(None, few, few), "");
    }

    #[test]
    fn a_file_takes_its_key_from_its_name_or_the_nearest_folder_that_names_one() {
        let path = Path::new("/packs/Gospel Pack/C Minor/Loop 01.mid");
        let folders = folders_above(path);
        assert_eq!(folders, ["packs", "Gospel Pack", "C Minor"]);
        assert_eq!(packs::label("Loop 01", &folders), (Named::Key(key("Cm")), "folder name"));
        assert_eq!(
            shown(path, Path::new("/packs/Gospel Pack")),
            ["Gospel Pack", "C Minor", "Loop 01.mid"].join(MAIN_SEPARATOR_STR)
        );
    }

    /// A one-track MIDI file at 96 ticks a beat: each chord held a bar on channel 1, the whole played twice.
    fn chords_smf(chords: &[&[u8]], name: &str) -> Vec<u8> {
        let mut track = Vec::new();
        track.extend([0x00, 0xFF, 0x03, name.len() as u8]);
        track.extend(name.as_bytes());
        for chord in chords.iter().chain(chords) {
            for &k in *chord {
                track.extend([0x00, 0x90, k, 100]);
            }
            // A bar later (384 ticks), every note of the chord let go.
            for (i, &k) in chord.iter().enumerate() {
                if i == 0 {
                    track.extend([0x83, 0x00]);
                } else {
                    track.push(0x00);
                }
                track.extend([0x80, k, 0]);
            }
        }
        track.extend([0x00, 0xFF, 0x2F, 0x00]);
        let mut out = b"MThd".to_vec();
        out.extend([0, 0, 0, 6, 0, 0, 0, 1, 0, 96]);
        out.extend(b"MTrk");
        out.extend((track.len() as u32).to_be_bytes());
        out.extend(track);
        out
    }

    #[test]
    fn both_finders_read_each_part_and_the_new_one_as_the_midi_page_keeps_it() {
        let path = Path::new("/packs/Pop Pack/Chords C major.mid");
        // C G Am F, a bar each: version 4 hears C major, version 5 the pair with A minor the likelier.
        let raw = chords_smf(&[&[60, 64, 67], &[55, 59, 62], &[57, 60, 64], &[53, 57, 60]], "Chords");
        let found = extract::from_bytes(path, &raw, &[]).unwrap();
        let (lines, drums) = lines_of(&found, "Chords C major");
        assert_eq!((lines.len(), drums), (1, 0));
        let line = &lines[0];
        assert_eq!(line.part, "chords, Chords");
        assert_eq!(line.old.key(), Some(key("C")), "{line:?}");
        assert_eq!(line.new.answer, KeyAnswer::OrRelative(key("Am")), "{line:?}");
        assert_eq!(mark(Some(key("C")), line.old, line.new), "worse");
        // The new finder's answer is the one the MIDI page keeps for the part.
        let kept = found[0].analysis.key_fact.unwrap();
        assert_eq!(kept.answer(), Some(line.new.answer));
        assert!((kept.shown_chance() - line.new.chance).abs() < 1e-4, "{kept:?} {line:?}");
    }

    fn checked(named: Named, lines: Vec<Line>) -> Checked {
        Checked { shown: "Pack/Loop.mid".into(), named, from_folder: false, read: Ok((lines, 1)) }
    }

    fn line(old: KeyAnswer, new: KeyAnswer) -> Line {
        Line { part: "chords".into(), cut: false, old: said(old, 0.9), new: said(new, 0.9) }
    }

    /// The cells of the table's line that starts with `label`.
    fn cells<'a>(text: &'a str, label: &str) -> Vec<&'a str> {
        text.lines().find_map(|l| l.strip_prefix(label)).unwrap_or_default().split_whitespace().collect()
    }

    #[test]
    fn the_counts_say_how_often_each_finder_agrees_with_the_names() {
        let (c, am) = (key("C"), key("Am"));
        let files = [
            // A C major loop the new finder hears as A minor or C major, and one both hear as C major.
            checked(Named::Key(c), vec![line(KeyAnswer::Key(c), KeyAnswer::OrRelative(am))]),
            checked(Named::Key(c), vec![line(KeyAnswer::Key(c), KeyAnswer::Key(c))]),
            // An A minor piece cut from a longer part, only the new finder gets.
            checked(Named::Key(am), vec![Line { cut: true, ..line(KeyAnswer::OrRelative(c), KeyAnswer::Key(am)) }]),
            // No key in the name; a file that couldn't be read.
            checked(Named::Nothing, vec![line(KeyAnswer::Key(c), KeyAnswer::Key(am))]),
            Checked {
                shown: "Pack/Broken.mid".into(),
                named: Named::Nothing,
                from_folder: false,
                read: Err("Damaged".into()),
            },
        ];
        let mut tally = Tally::default();
        for f in &files {
            tally.add(f);
        }
        assert_eq!((tally.files, tally.unread, tally.parts, tally.drums), (5, 1, 4, 4));
        assert_eq!((tally.changed, tally.no_key, tally.unclear, tally.better, tally.worse), (3, 1, 0, 1, 1));
        let [every, minor, major] = tally.groups;
        assert_eq!((every.parts, minor.parts, major.parts), (3, 1, 2));
        assert_eq!(every.old, Counts { agrees: 2, sure: 2, sure_right: 2, pair: 1, pair_right: 1 });
        assert_eq!(every.new, Counts { agrees: 2, sure: 2, sure_right: 2, pair: 1, pair_right: 1 });
        assert_eq!((major.old.agrees, major.new.agrees, minor.old.agrees, minor.new.agrees), (2, 1, 0, 1));
        assert_eq!(tally.loops.map(|g| g.parts), [2, 0, 2]);
        assert_eq!(tally.pieces.map(|g| g.parts), [1, 1, 0]);

        let mut out = Vec::new();
        report(&files, Show::Changed, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        // Only the parts that changed, each file's name once with them.
        assert!(text.contains("Pack/Loop.mid: the name says C major\n  chords"), "{text}");
        assert!(text.contains("old C major 90%"), "{text}");
        assert!(text.contains("new A minor or C major 90%"), "{text}");
        assert_eq!(text.matches(" worse\n").count(), 1, "{text}");
        assert_eq!(text.matches(" better\n").count(), 1, "{text}");
        assert!(!text.contains("Broken"), "{text}");
        assert!(text.contains("5 MIDI files: 4 parts read, besides 4 drum parts. 1 file couldn't be read."), "{text}");
        assert!(text.contains("differently on 3 of them (75.0%)"), "{text}");
        assert!(text.contains("3 parts in files whose names give one key (1 minor, 2 major)."), "{text}");
        // Every name, then the minor ones (the A minor piece), then the major ones (the two C major loops).
        assert_eq!(
            cells(&text, "Gives the name's key, or leans to it"),
            ["66.7%", "66.7%", "0.0%", "100.0%", "100.0%", "50.0%"]
        );
        assert_eq!(cells(&text, "  of the whole loops"), ["100.0%", "50.0%", "-", "-", "100.0%", "50.0%"]);
        assert_eq!(
            cells(&text, "  of the pieces cut from longer parts"),
            ["0.0%", "100.0%", "0.0%", "100.0%", "-", "-"]
        );
        // The pairs: version 4's on the A minor piece (C major or A minor), version 5's on the first C major loop (A
        // minor or C major), the name's key one of the two each time.
        assert_eq!(
            cells(&text, "  the name's one of the two, of those"),
            ["100.0%", "100.0%", "100.0%", "-", "-", "100.0%"]
        );
        assert_eq!(cells(&text, "Parts"), ["3", "1", "2"]);
        assert!(text.contains("Better on 1 part: the new finder (version 5) gives the name's key"), "{text}");
        assert!(text.contains("Worse on 1: the old one does, and the new one doesn't."), "{text}");

        let mut out = Vec::new();
        report(&files, Show::Counts, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(!text.contains("Pack/Loop.mid"), "only the counts: {text}");
        assert!(text.contains("Of the parts whose names give one key:"), "{text}");
    }
}
