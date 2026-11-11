//! `theorybench songkeys`: the key finder on Solomon's own songs, whole, before the Songs page shows their keys. The
//! finder was fitted on songs from public datasets and then rescaled on his MIDI packs (version 5), never on his own
//! projects, so this measures it there: each song's key as he typed it on the Songs page (and, apart, the key a song's
//! name gives), beside what the old app found (`detected_key`), version 4 (before the rescale) and the one built in.
//! It reads the song library and the projects, and prints; it writes nothing.
//!
//! A song is heard as it plays: every pitched channel or track, its notes where the arrangement puts them (FL Studio's
//! playlist, Bitwig's arranger, or its scenes one after another when the arranger is empty, Studio One's arrangement),
//! muted clips and tracks left out. Drums aren't heard: a drum plugin's track (FPC, Battery, Drum Machine...), a
//! track whose notes and names say drums (as the MIDI page tells), and in FL Studio a channel that plays a sample
//! rather than an instrument, whose pitch the notes alone don't tell (as the old app left them out). It's read as a
//! whole song ([`Span::Song`]).
//!
//! Each song is heard a second way too, as the MIDI page hears a project: each pattern or clip once, one after
//! another, however often the arrangement plays it ("patterns"). The two ways say which the Songs page should use.

use std::collections::HashMap;
use std::io::Write;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};

use mo_formats::arrangement::{self, LaneKind};
use mo_formats::{NoteAttribute, NoteEvent, fl};
use mo_harmony::keyfind::{self, KeyAnswer, KeyReader, KeyReading, Model, Span, Why, cases, numbers_hash};
use mo_harmony::{Grid, Key, KeyRelation, Note, Part};
use mo_library::Library;
use mo_midi::analyser::{self, Shape};
use mo_midi::extract::{self, is_unpitched_plugin};
use mo_midi::{PPQ, Role};
use rayon::prelude::*;

use crate::keyfind_v4;
use crate::packs::{self, Named};

/// What one finder (or the old app) said about a song.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Said {
    /// The finder's answer and the chance it gives that it's right.
    Read { answer: KeyAnswer, chance: f64 },
    /// The old app's key, as the library holds it.
    Old(Key),
    /// No answer: the old app found none, or the song has no pitched notes.
    Nothing,
}

impl Said {
    fn of(reading: &KeyReading) -> Said {
        Said::Read { answer: reading.answer, chance: reading.shown_chance }
    }

    /// The key it gives, or leans to when it isn't sure.
    fn key(self) -> Option<Key> {
        match self {
            Said::Read { answer, .. } => answer.leaning_key(),
            Said::Old(key) => Some(key),
            Said::Nothing => None,
        }
    }

    /// In a few words: "C minor 91%", "A minor or C major 72%", "unsure, leaning F minor 40%", "-".
    fn words(self) -> String {
        match self {
            Said::Read { answer, chance } => {
                let chance = format!("{:.0}%", chance * 100.0);
                match answer {
                    KeyAnswer::Key(_) | KeyAnswer::OrRelative(_) | KeyAnswer::HomeNote { .. } => {
                        format!("{} {chance}", cases::describe(answer))
                    }
                    KeyAnswer::Unclear { leaning: Some(k), .. } => format!("unsure, leaning {k} {chance}"),
                    KeyAnswer::Unclear { why: Why::TooFewNotes, .. } => "too few notes".to_owned(),
                    KeyAnswer::Unclear { why: Why::NotTonal, .. } => "not in one scale".to_owned(),
                    KeyAnswer::Unclear { why: Why::Spread, .. } => "no clear key".to_owned(),
                }
            }
            Said::Old(key) => key.name(),
            Said::Nothing => "-".to_owned(),
        }
    }
}

/// The four answers for a song, in the order the table shows them.
const WHO: [&str; 4] = ["old app", "version 4", "built in", "patterns"];

/// One song, read.
struct Song {
    name: String,
    daw: String,
    /// The key typed on the Songs page, when it's one that can be read; the text when it isn't.
    typed: Result<Option<Key>, String>,
    /// The key the song's name gives.
    named: Named,
    /// The key the names inside it give: its samples' and loops' file names, its patterns', clips' and tracks' names
    /// ("Melody Cm 140.wav"), when they all name the same one.
    inside: Named,
    /// The old app's key.
    old: Option<Key>,
    /// The old app's, version 4's and the built-in finder's answers, as the song plays; and the built-in one's with
    /// each pattern heard once. Or why the song couldn't be read.
    said: Result<[Said; 4], String>,
    /// Bars of pitched music heard as the song plays, and the kinds of part heard.
    bars: u32,
    parts: String,
}

/// What a song's key is taken from, to score the answers against. Each is a table of its own, and a song can be in
/// more than one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    /// The key typed on the Songs page.
    Typed,
    /// The key the song's own name gives.
    Name,
    /// The key the names inside it give ([`Song::inside`]).
    Inside,
    /// The old app's key: not a right answer, only one to agree with or not.
    OldApp,
}

/// Each source: its table's title, and how a song's line says where its key came from.
const SOURCES: [(Source, &str, &str); 4] = [
    (Source::Typed, "Songs with a key you typed", "you typed"),
    (Source::Name, "Songs whose own name gives one key", "its name says"),
    (
        Source::Inside,
        "Songs whose samples, loops and patterns name one key (all the same one)",
        "its samples and patterns say",
    ),
    (
        Source::OldApp,
        "Agreement with the old app, on every song it found a key for (the same key or not: neither is known right)",
        "the old app found",
    ),
];

impl Song {
    /// The key it's scored against, from this source.
    fn truth(&self, source: Source) -> Option<Key> {
        let named = |n: Named| if let Named::Key(k) = n { Some(k) } else { None };
        match source {
            Source::Typed => self.typed.clone().ok().flatten(),
            Source::Name => named(self.named),
            Source::Inside => named(self.inside),
            Source::OldApp => self.old,
        }
    }
}

/// The one key a set of names gives: [`Named::Key`] when every name that gives a key gives the same one,
/// [`Named::Unclear`] when they give different keys, [`Named::Nothing`] when none gives one.
fn one_key<'a>(names: impl IntoIterator<Item = &'a str>) -> Named {
    let mut keys: Vec<Key> = names
        .into_iter()
        .filter_map(|name| match packs::label(name, &[]).0 {
            Named::Key(k) => Some(k),
            Named::Nothing | Named::Unclear => None,
        })
        .collect();
    keys.sort_by_key(|k| (k.tonic.number(), k.is_minor()));
    keys.dedup();
    match keys.as_slice() {
        [] => Named::Nothing,
        [k] => Named::Key(*k),
        _ => Named::Unclear,
    }
}

/// How the key finder hears a part of this kind (as the MIDI page does, `mo_midi::keys`).
fn heard_as(role: Role) -> mo_harmony::Role {
    match role {
        Role::Melody => mo_harmony::Role::Melody,
        Role::Chords | Role::Arp => mo_harmony::Role::Harmony,
        Role::Bass => mo_harmony::Role::Bass,
        Role::Drums => mo_harmony::Role::Drums,
        Role::Other => mo_harmony::Role::Unknown,
    }
}

/// A track's notes (at `ppq`, where they play in the song) as a part, its kind guessed from its notes and names as the
/// MIDI page guesses it; none for drums or no notes.
fn part(notes: &[NoteEvent], ppq: i64, hints: &str, drums: bool) -> Option<(Role, Vec<Note>)> {
    let notes = analyser::normalise(notes, ppq, true);
    if notes.is_empty() {
        return None;
    }
    let (role, _) = analyser::role(&Shape::of(&notes), &notes, hints, drums);
    if role == Role::Drums {
        return None;
    }
    let notes = notes.iter().map(|n| Note { start: n.start, length: n.length, key: n.key, velocity: n.velocity });
    Some((role, notes.collect()))
}

/// An FL Studio song's pitched channels, as the playlist plays them.
fn fl_parts(raw: &[u8]) -> Result<Vec<(Role, Vec<Note>)>, String> {
    let flp = fl::events::load(raw).map_err(|_| "not an FL Studio project, or damaged".to_owned())?;
    let song = fl::song::read_song_with(&flp, false);
    let ppq = i64::from(song.ppq.max(1));
    Ok(song
        .channels
        .iter()
        .filter(|c| c.enabled)
        .filter_map(|c| {
            let plugin = c.instrument.as_ref().map(|p| p.name.as_str()).unwrap_or_default();
            // A sample plays at its own pitch, which the notes don't tell; a channel with no instrument plays one.
            let drums = plugin.is_empty() || is_unpitched_plugin(plugin) || is_unpitched_plugin(&c.name);
            part(&c.notes, ppq, &format!("{} {plugin}", c.name), drums)
        })
        .collect())
}

/// A Bitwig or Studio One song's instrument tracks, as its arrangement plays them (the first view: Bitwig's arranger,
/// or its scenes when the arranger is empty).
fn arranged_parts(path: &Path, raw: &[u8]) -> Result<Vec<(Role, Vec<Note>)>, String> {
    let views = arrangement::read_bytes(path, raw);
    let Some(view) = views.first() else { return Err("not a project this reads".into()) };
    // A project too damaged to draw gives an empty view that says so.
    if view.lanes.is_empty() && view.notes.iter().any(|n| n.contains("couldn't be read")) {
        return Err(view.notes.join(" "));
    }
    let beat = PPQ as f64;
    let mut out = Vec::new();
    for lane in view.lanes.iter().filter(|l| l.kind == LaneKind::Instrument && !l.muted) {
        let mut notes = Vec::new();
        for clip in lane.clips.iter().filter(|c| !c.muted) {
            for n in clip.notes.iter().filter(|n| !n.muted && n.start < clip.length && n.start >= 0.0) {
                notes.push(NoteEvent {
                    start: ((clip.start + n.start) * beat).round() as i64,
                    length: (n.length.min(clip.length - n.start) * beat).round().max(1.0) as i64,
                    key: n.key,
                    velocity: n.velocity,
                    muted: false,
                    step: n.step,
                    attribute: NoteAttribute::NONE,
                });
            }
        }
        // Studio One's detail says where the track goes too: "Battery 4, to Drums".
        let plugin = lane.detail.split(", to ").next().unwrap_or_default();
        let drums = is_unpitched_plugin(plugin) || is_unpitched_plugin(&lane.name);
        out.extend(part(&notes, PPQ, &format!("{} {}", lane.name, lane.detail), drums));
    }
    Ok(out)
}

/// Each pattern or clip heard once, as the MIDI page finds them (`extract::from_bytes`), one after another: none
/// without pitched notes.
fn patterns_heard(reader: &mut KeyReader, found: &[extract::Found]) -> Option<keyfind::Heard> {
    let mut all: Option<keyfind::Heard> = None;
    for f in found.iter().filter(|f| !f.drums && f.analysis.role != Role::Drums) {
        let Some((heard, _)) = mo_midi::keys::hear(reader, &f.notes, f.analysis.role, f.cut) else { continue };
        match &mut all {
            Some(all) => all.merge(&heard),
            None => all = Some(heard),
        }
    }
    all
}

/// The kinds of part heard, in a few letters: "melody, chords x3, bass".
fn kinds(parts: &[(Role, Vec<Note>)]) -> String {
    let mut counts: Vec<(Role, usize)> = Vec::new();
    for (role, _) in parts {
        match counts.iter_mut().find(|(r, _)| r == role) {
            Some((_, n)) => *n += 1,
            None => counts.push((*role, 1)),
        }
    }
    let words: Vec<String> = counts
        .iter()
        .map(|(r, n)| if *n == 1 { r.name().to_lowercase() } else { format!("{} x{n}", r.name().to_lowercase()) })
        .collect();
    words.join(", ")
}

/// Reads one song's project and hears it both ways, with both finders.
fn read_song(path: &Path) -> Result<([Said; 3], u32, String, Vec<String>), String> {
    let raw = std::fs::read(path).map_err(|e| format!("couldn't be opened: {e}"))?;
    let read = mo_catch::catch(AssertUnwindSafe(|| {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
        let parts = match ext.as_str() {
            "flp" => fl_parts(&raw)?,
            "bwproject" | "song" => arranged_parts(path, &raw)?,
            _ => return Err("not an FL Studio, Bitwig or Studio One project".to_owned()),
        };
        // Found first: finding the parts reads each one's key with this thread's own key finder.
        let found = extract::from_bytes(path, &raw, &[]).unwrap_or_default();
        mo_midi::keys::with_reader(|reader| {
            let pieces: Vec<Part> =
                parts.iter().map(|(role, notes)| Part { role: heard_as(*role), notes: notes.as_slice() }).collect();
            let played = reader.hear(&pieces, Grid::four_four(PPQ as u32));
            let bars = played.as_ref().map_or(0, keyfind::Heard::bars_heard);
            let read = |heard: Option<&keyfind::Heard>, model: &Model| {
                heard.map_or(Said::Nothing, |h| Said::of(&keyfind::read_heard(h, Span::Song, model)))
            };
            let patterns = patterns_heard(reader, &found);
            // The names of its patterns, clips and tracks, a piece's " · bars 9–16" left off.
            let mut names: Vec<String> = found
                .iter()
                .flat_map(|f| [&f.part, &f.clip, &f.track_name])
                .map(|n| n.split(" · ").next().unwrap_or_default().to_owned())
                .filter(|n| !n.is_empty())
                .collect();
            names.sort();
            names.dedup();
            Ok((
                [
                    read(played.as_ref(), &keyfind_v4::MODEL),
                    read(played.as_ref(), &keyfind::SHIPPED),
                    read(patterns.as_ref(), &keyfind::SHIPPED),
                ],
                bars,
                kinds(&parts),
                names,
            ))
        })
    }));
    read.unwrap_or_else(|_| Err("the reader stopped on it: it looks damaged".into()))
}

/// How one finder came out on a set of songs, against their keys.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Counts {
    /// It gives the key, or leans to it.
    agrees: usize,
    /// Its key, or the key it leans to, is the right key's relative, a fifth away (same mode), the same home note in
    /// the other mode, or something else; or it gives none.
    relative: usize,
    fifth: usize,
    parallel: usize,
    other: usize,
    none: usize,
    /// The MIREX score, added up (1 the key, 0.5 a fifth either way, 0.3 the relative, 0.2 the parallel).
    mirex: f64,
    /// It gives one key, sure ...
    sure: usize,
    /// ... and it's the right one.
    sure_right: usize,
    /// It gives a key or its relative, and one of the two is right.
    pair: usize,
    pair_right: usize,
}

impl Counts {
    fn add(&mut self, said: Said, truth: Key) {
        match said.key() {
            None => self.none += 1,
            Some(k) => {
                self.mirex += k.mirex_score(truth, true);
                match k.relation_to(truth) {
                    KeyRelation::Same => self.agrees += 1,
                    KeyRelation::Relative => self.relative += 1,
                    KeyRelation::FifthAbove | KeyRelation::FifthBelow => self.fifth += 1,
                    KeyRelation::Parallel => self.parallel += 1,
                    KeyRelation::Other => self.other += 1,
                }
            }
        }
        if let Said::Read { answer, .. } = said {
            match answer {
                KeyAnswer::Key(k) => {
                    self.sure += 1;
                    self.sure_right += usize::from(k == truth);
                }
                KeyAnswer::OrRelative(k) => {
                    self.pair += 1;
                    self.pair_right += usize::from(k == truth || k.relative() == truth);
                }
                KeyAnswer::HomeNote { .. } | KeyAnswer::Unclear { .. } => {}
            }
        }
    }
}

/// The songs of one kind of key (every key, the minor ones or the major ones), for each of [`WHO`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Group {
    songs: usize,
    who: [Counts; 4],
}

impl Group {
    fn add(&mut self, said: &[Said; 4], truth: Key) {
        self.songs += 1;
        for (counts, said) in self.who.iter_mut().zip(said) {
            counts.add(*said, truth);
        }
    }
}

/// The old app's key for a song: the library's `detected_key`, as it names keys ("F minor").
fn old_key(text: &str) -> Said {
    Key::parse(text).map_or(Said::Nothing, Said::Old)
}

/// A share as a percentage, or "-" of nothing.
fn share(n: usize, of: usize) -> String {
    if of == 0 { "-".to_owned() } else { format!("{:.1}%", 100.0 * n as f64 / of as f64) }
}

/// "1 song", "2 songs".
fn plural(n: usize, one: &str) -> String {
    if n == 1 { format!("1 {one}") } else { format!("{n} {one}s") }
}

/// One line of a table: the cell of one group for one of [`WHO`].
type Row = (&'static str, fn(&Group, &Counts) -> String);

const ROWS: [Row; 11] = [
    ("Gives the key, or leans to it", |g, c| share(c.agrees, g.songs)),
    ("MIREX score (fifths both ways)", |g, c| {
        if g.songs == 0 { "-".into() } else { format!("{:.1}%", 100.0 * c.mirex / g.songs as f64) }
    }),
    ("  the relative key", |g, c| share(c.relative, g.songs)),
    ("  a fifth away", |g, c| share(c.fifth, g.songs)),
    ("  same home note, other mode", |g, c| share(c.parallel, g.songs)),
    ("  another key", |g, c| share(c.other, g.songs)),
    ("  no key at all", |g, c| share(c.none, g.songs)),
    ("Sure of one key", |g, c| share(c.sure, g.songs)),
    ("  right, of those", |_, c| share(c.sure_right, c.sure)),
    ("A key or its relative", |g, c| share(c.pair, g.songs)),
    ("  one of the two right, of those", |_, c| share(c.pair_right, c.pair)),
];

/// One table: every key, the minor ones and the major ones, each of [`WHO`] in a column.
fn table(title: &str, groups: &[Group; 3], out: &mut impl Write) -> std::io::Result<()> {
    let [every, minor, major] = groups;
    writeln!(out, "\n{title}: {} ({} minor, {} major)", plural(every.songs, "song"), minor.songs, major.songs)?;
    if every.songs == 0 {
        return Ok(());
    }
    for (name, group) in [("Every key", every), ("Minor keys", minor), ("Major keys", major)] {
        if group.songs == 0 {
            continue;
        }
        writeln!(out, "  {name} ({}):", plural(group.songs, "song"))?;
        let head: String = WHO.iter().map(|w| format!("{w:<11}")).collect();
        writeln!(out, "  {:<36}{}", "", head.trim_end())?;
        for (label, cell) in ROWS {
            let cells: String = group.who.iter().map(|c| format!("{:<11}", cell(group, c))).collect();
            writeln!(out, "  {}", format!("{label:<36}{cells}").trim_end())?;
        }
    }
    Ok(())
}

/// What was asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Show {
    /// Every song with a key.
    All,
    /// Only the songs the built-in finder misses.
    Misses,
    /// Only the counts.
    Counts,
}

/// Prints the songs (as `show` says) and the tables.
fn report(songs: &[Song], show: Show, out: &mut impl Write) -> std::io::Result<()> {
    let built_in = format!("version {}", mo_harmony::KEY_FINDER_VERSION);
    writeln!(
        out,
        "Old app: the key the old app found, as the library holds it. Version {}: the finder before the rescale on \
         your packs. Built in: {built_in}, as the song plays. Patterns: {built_in}, each pattern or clip heard once.",
        keyfind_v4::VERSION
    )?;
    if numbers_hash(&keyfind::SHIPPED) == numbers_hash(&keyfind_v4::MODEL) {
        writeln!(out, "The finder built in has version {}'s numbers, so the two say the same.", keyfind_v4::VERSION)?;
    }
    let mut groups = [[Group::default(); 3]; SOURCES.len()];
    let (mut unread, mut silent, mut odd_typed, mut better, mut worse) = (0, 0, Vec::new(), 0, 0);
    let mut by_daw: HashMap<String, [usize; 3]> = HashMap::new();
    for song in songs {
        if let Err(text) = &song.typed {
            odd_typed.push(text.clone());
        }
        let said = match &song.said {
            Ok(said) => said,
            Err(why) => {
                unread += 1;
                if show == Show::All {
                    writeln!(out, "{} ({}): couldn't be read: {why}", song.name, song.daw)?;
                }
                continue;
            }
        };
        if said[1] == Said::Nothing && said[3] == Said::Nothing {
            silent += 1;
        }
        for ((source, _, from), groups) in SOURCES.iter().zip(&mut groups) {
            let Some(truth) = song.truth(*source) else { continue };
            for g in [0, if truth.is_minor() { 1 } else { 2 }] {
                groups[g].add(said, truth);
            }
            if *source == Source::OldApp {
                // Thousands of songs: counted, not listed.
                continue;
            }
            if *source == Source::Typed {
                let daw = by_daw.entry(song.daw.clone()).or_default();
                daw[0] += 1;
                daw[1] += usize::from(said[1].key() == Some(truth));
                daw[2] += usize::from(said[2].key() == Some(truth));
                match (said[1].key() == Some(truth), said[2].key() == Some(truth)) {
                    (false, true) => better += 1,
                    (true, false) => worse += 1,
                    _ => {}
                }
            }
            let missed = said[2].key() != Some(truth);
            if show == Show::All || show == Show::Misses && missed {
                writeln!(
                    out,
                    "{} ({}, {} bars: {}): {from} {truth}\n    old app {:<24} v{} {:<30} built in {:<30} patterns {}",
                    song.name,
                    song.daw,
                    song.bars,
                    if song.parts.is_empty() { "no pitched parts" } else { &song.parts },
                    said[0].words(),
                    keyfind_v4::VERSION,
                    said[1].words(),
                    said[2].words(),
                    said[3].words(),
                )?;
            }
        }
    }
    writeln!(
        out,
        "\n{} in the library: {unread} couldn't be read, {silent} have no pitched notes.",
        plural(songs.len(), "song")
    )?;
    if !odd_typed.is_empty() {
        odd_typed.sort();
        odd_typed.dedup();
        writeln!(out, "Typed keys that aren't a key it can read (left out): {}", odd_typed.join(", "))?;
    }
    for ((source, title, _), groups) in SOURCES.iter().zip(&groups) {
        table(title, groups, out)?;
        if *source != Source::Typed || groups[0].songs == 0 {
            continue;
        }
        let mut daws: Vec<_> = by_daw.iter().collect();
        daws.sort();
        for (daw, [n, old, new]) in daws {
            writeln!(
                out,
                "  {daw}: {n}; version {} {}, built in {}",
                keyfind_v4::VERSION,
                share(*old, *n),
                share(*new, *n)
            )?;
        }
        writeln!(
            out,
            "  Built in better on {}: it gives your key where version {} doesn't. Worse on {worse}.",
            plural(better, "song"),
            keyfind_v4::VERSION
        )?;
    }
    Ok(())
}

/// What was asked for: the library, and which songs to print.
fn asked(args: &[String]) -> Result<(PathBuf, Show), String> {
    let (mut library, mut show) = (None, Show::All);
    for arg in crate::keycheck::as_typed(args, cfg!(windows)) {
        match arg.as_str() {
            "--misses" => show = Show::Misses,
            "--counts" => show = Show::Counts,
            flag if flag.starts_with("--") => return Err(crate::USAGE.to_owned()),
            given if library.is_none() => library = Some(PathBuf::from(given)),
            _ => return Err(crate::USAGE.to_owned()),
        }
    }
    let library = library.ok_or("songkeys needs the song library: songs.db (or the old app's library.db)")?;
    Ok((library, show))
}

pub fn run(args: &[String]) -> Result<(), String> {
    let (library, show) = asked(args)?;
    let opened = Library::open_read_only(&library).map_err(|e| e.to_string())?;
    let saves = opened.saves().map_err(|e| e.to_string())?;
    let mut samples = opened.sample_paths().map_err(|e| e.to_string())?;
    drop(opened);
    let saves: Vec<_> = saves
        .into_iter()
        .filter(|s| !s.file_missing)
        .map(|s| {
            let used = samples.remove(&s.id).unwrap_or_default();
            (s, used)
        })
        .collect();
    println!("Reading {}...", plural(saves.len(), "song"));
    let songs: Vec<Song> = saves
        .into_par_iter()
        .map(|(s, used)| {
            let typed = match s.musical_key.trim() {
                "" => Ok(None),
                text => Key::parse(text).map(Some).ok_or_else(|| text.to_owned()),
            };
            let stem = |p: &str| mo_formats::paths::stem(mo_formats::paths::file_name(p)).to_owned();
            let (named, _) = packs::label(&stem(&s.path), &[]);
            let (said, bars, parts, names) = match read_song(Path::new(&s.path)) {
                Ok(([v4, built_in, patterns], bars, parts, names)) => {
                    (Ok([old_key(&s.detected_key), v4, built_in, patterns]), bars, parts, names)
                }
                Err(why) => (Err(why), 0, String::new(), Vec::new()),
            };
            // A sample path may be Windows' (C:\\...) read anywhere: cut at either slash.
            let sample_stems: Vec<String> =
                used.iter().map(|p| stem(p.rsplit(['\\', '/']).next().unwrap_or(p))).collect();
            let inside = one_key(sample_stems.iter().chain(&names).map(String::as_str));
            let old = Key::parse(&s.detected_key);
            Song { name: s.name, daw: s.daw, typed, named, inside, old, said, bars, parts }
        })
        .collect();
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    match report(&songs, show, &mut out).and_then(|()| out.flush()) {
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        result => result.map_err(|e| e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(beat: f64, beats: f64, key: u8) -> NoteEvent {
        NoteEvent {
            start: (beat * PPQ as f64) as i64,
            length: (beats * PPQ as f64) as i64,
            key,
            velocity: 40_000,
            muted: false,
            step: false,
            attribute: NoteAttribute::NONE,
        }
    }

    /// C major's I IV V I, four times over, with a bass: read as C major by the finder built in, as a song.
    #[test]
    fn a_plain_song_in_c_major_reads_c_major() {
        let chords: [[u8; 3]; 4] = [[60, 64, 67], [60, 65, 69], [59, 62, 67], [60, 64, 67]];
        let (mut keys, mut bass) = (Vec::new(), Vec::new());
        for round in 0..4 {
            for (i, chord) in chords.iter().enumerate() {
                let at = ((round * 4 + i) * 4) as f64;
                keys.extend(chord.iter().map(|&k| note(at, 4.0, k)));
                bass.push(note(at, 4.0, [36, 41, 43, 36][i]));
            }
        }
        let parts: Vec<(Role, Vec<Note>)> = [(keys, "Piano"), (bass, "Bass")]
            .iter()
            .filter_map(|(notes, name)| part(notes, PPQ, name, false))
            .collect();
        assert_eq!(parts.len(), 2);
        let pieces: Vec<Part> = parts.iter().map(|(r, n)| Part { role: heard_as(*r), notes: n }).collect();
        let heard = KeyReader::new().hear(&pieces, Grid::four_four(PPQ as u32)).expect("pitched notes");
        let reading = keyfind::read_heard(&heard, Span::Song, &keyfind::SHIPPED);
        assert_eq!(reading.answer.leaning_key(), Some(Key::major(mo_harmony::PitchClass::C)));
    }

    #[test]
    fn a_drum_plugin_is_not_heard() {
        let notes = vec![note(0.0, 1.0, 36), note(1.0, 1.0, 38), note(2.0, 1.0, 36), note(3.0, 1.0, 38)];
        assert!(part(&notes, PPQ, "Kit FPC", true).is_none());
        assert!(part(&[], PPQ, "Lead", false).is_none());
    }

    #[test]
    fn misses_are_counted_by_how_they_stand_to_the_key() {
        let a_minor = Key::parse("Am").unwrap();
        let mut c = Counts::default();
        c.add(Said::Old(Key::parse("C").unwrap()), a_minor);
        c.add(Said::Old(Key::parse("Em").unwrap()), a_minor);
        c.add(Said::Old(Key::parse("A").unwrap()), a_minor);
        c.add(Said::Old(a_minor), a_minor);
        c.add(Said::Nothing, a_minor);
        assert_eq!((c.agrees, c.relative, c.fifth, c.parallel, c.none), (1, 1, 1, 1, 1));
        assert!((c.mirex - (1.0 + 0.3 + 0.5 + 0.2)).abs() < 1e-9);
    }

    #[test]
    fn the_old_apps_key_is_read_as_it_wrote_it() {
        assert_eq!(old_key("F minor"), Said::Old(Key::parse("Fm").unwrap()));
        assert_eq!(old_key("Bb major"), Said::Old(Key::parse("Bb").unwrap()));
        assert_eq!(old_key(""), Said::Nothing);
    }

    #[test]
    fn each_source_gives_its_own_key() {
        let (am, c) = (Key::parse("Am").unwrap(), Key::parse("C").unwrap());
        let song = Song {
            name: String::new(),
            daw: String::new(),
            typed: Ok(Some(am)),
            named: Named::Key(c),
            inside: Named::Unclear,
            old: Some(c),
            said: Err(String::new()),
            bars: 0,
            parts: String::new(),
        };
        let truths = [Source::Typed, Source::Name, Source::Inside, Source::OldApp].map(|s| song.truth(s));
        assert_eq!(truths, [Some(am), Some(c), None, Some(c)]);
        let odd = Song { typed: Err("Dorian".into()), ..song };
        assert_eq!(odd.truth(Source::Typed), None);
    }

    #[test]
    fn the_names_inside_a_song_give_a_key_only_when_they_agree() {
        let cm = Key::parse("Cm").unwrap();
        assert_eq!(one_key(["Melody Cm 140", "Kick 01", "Pattern 3"]), Named::Key(cm));
        assert_eq!(one_key(["Melody Cm 140", "Pad C minor"]), Named::Key(cm));
        assert_eq!(one_key(["Melody Cm 140", "Bass Fm"]), Named::Unclear);
        assert_eq!(one_key(["Kick 01", "Snare"]), Named::Nothing);
    }
}
