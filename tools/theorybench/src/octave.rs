//! `theorybench octave`: how often the app puts a part back at the octave it was written at
//! (`mo_midi::fit::octave`), beside the C# app's rule (`HarmonyMatch.OctavesFor`, ported here as it was).
//!
//! A part found for a song is moved into the song's key by the search; the octave it then goes at is a rule's guess.
//! Here every part of every song is cut into stretches of a few bars (the songs and the stretches `pairs` takes), as
//! a part found in a library is a few bars long, and each stretch is asked about as if it had been found for the
//! rest of its song: a rule that moves it by no octaves has put it back where its writer had it. A stretch a whole
//! number of octaves away would be asked the same thing (every rule goes by the stretch's middle), so only the
//! stretch as written is tried.
//!
//! Nothing is fitted: the rule's few numbers are the C# app's, or chosen by hand from what this prints on folds 1 to
//! 4. Fold 0 is kept back as for the other measures (`--folds 0`, a line in `test-runs.md`).

use std::path::PathBuf;

use mo_formats::{NoteAttribute, NoteEvent};
use mo_midi::fit::octave::{MOST_OCTAVES, Registers, edges, middle, octaves_to, stands_in, usual};
use mo_midi::{PPQ, Role};

use crate::item::{self, Item, LEAST_BACKING, LEAST_TUNE};
use crate::keyfit;

/// One part of a song, as the app would have it.
struct Part {
    role: Role,
    /// Its notes as labelled, in order of start, for cutting stretches from.
    labelled: Vec<item::Note>,
    /// All of them as the app has notes.
    notes: Vec<NoteEvent>,
}

/// The kind of part a labelled role is to the app; none for drums and for parts the dataset says nothing of.
fn role_of(role: &str) -> Option<Role> {
    match role {
        "melody" => Some(Role::Melody),
        "accompaniment" | "chords" => Some(Role::Chords),
        "bass" => Some(Role::Bass),
        _ => None,
    }
}

/// Labelled notes as the app has notes, at its ticks a beat.
fn as_app(item: &Item, notes: &[item::Note]) -> Vec<NoteEvent> {
    let ticks = |t: i64| t * PPQ / i64::from(item.ppq.max(1));
    notes
        .iter()
        .map(|n| NoteEvent {
            start: ticks(n.start),
            length: ticks(n.length).max(1),
            key: n.key,
            velocity: u16::from(n.velocity) << 9,
            muted: false,
            step: false,
            attribute: NoteAttribute::default(),
        })
        .collect()
}

/// A song's pitched parts.
fn parts_of(item: &Item) -> Vec<Part> {
    item.parts
        .iter()
        .filter_map(|p| {
            let role = role_of(&p.role)?;
            let mut labelled: Vec<item::Note> = p
                .notes
                .iter()
                .map(|&(start, length, key, velocity)| item::Note { start, length, key, velocity })
                .collect();
            labelled.sort_by_key(|n| (n.start, n.key));
            Some(Part { role, notes: as_app(item, &labelled), labelled })
        })
        .collect()
}

/// The C# app's middle of a part (`HarmonyMatcher.Centre`): its keys' average, each note counting for its length,
/// 8 beats at the most (and a tick at the least).
fn old_centre(notes: &[NoteEvent]) -> Option<f64> {
    let (mut sum, mut all) = (0.0, 0.0);
    for n in notes {
        let ticks = n.length.clamp(1, 8 * PPQ) as f64;
        sum += f64::from(n.key) * ticks;
        all += ticks;
    }
    (all > 0.0).then(|| sum / all)
}

/// The C# app's usual places.
fn old_usual(role: Role) -> f64 {
    match role {
        Role::Bass => 40.0,
        Role::Chords => 60.0,
        Role::Arp => 67.0,
        Role::Melody => 72.0,
        Role::Drums | Role::Other => 62.0,
    }
}

/// The C# app's rule for a part of kind `role` whose centre is `centre`, beside `others`: next to the song's parts
/// of its kind when it has any (a bass by the lowest of them, the rest by their average); else at the kind's usual
/// place, nudged by how far the song's other kinds are from theirs (12 semitones at the most).
fn old_octaves(others: &[&Part], role: Role, centre: f64) -> i32 {
    let mut registers: Vec<(Role, f64)> = Vec::new();
    for kind in Role::ALL.into_iter().filter(|r| *r != Role::Drums) {
        let centres: Vec<f64> = others.iter().filter(|p| p.role == kind).filter_map(|p| old_centre(&p.notes)).collect();
        if centres.is_empty() {
            continue;
        }
        let lowest = centres.iter().copied().fold(f64::INFINITY, f64::min);
        let average = centres.iter().sum::<f64>() / centres.len() as f64;
        registers.push((kind, if kind == Role::Bass { lowest } else { average }));
    }
    let wanted = match registers.iter().find(|(kind, _)| *kind == role) {
        Some((_, own)) => *own,
        None if registers.is_empty() => old_usual(role),
        None => {
            let nudge = registers.iter().map(|(kind, at)| at - old_usual(*kind)).sum::<f64>() / registers.len() as f64;
            old_usual(role) + nudge.clamp(-12.0, 12.0)
        }
    };
    // C#'s `Math.Round` sends a half to the even number.
    (((wanted - centre) / 12.0).round_ties_even() as i32).clamp(-MOST_OCTAVES, MOST_OCTAVES)
}

/// How one rule did: how many stretches it left at their own octave, put one octave out, and further.
#[derive(Clone, Copy, Default)]
struct Count {
    own: usize,
    one_out: usize,
    further: usize,
}

impl Count {
    fn add(&mut self, octaves: i32) {
        match octaves.abs() {
            0 => self.own += 1,
            1 => self.one_out += 1,
            _ => self.further += 1,
        }
    }

    fn line(&self, name: &str) -> String {
        let all = (self.own + self.one_out + self.further).max(1) as f64;
        let share = |n: usize| 100.0 * n as f64 / all;
        format!(
            "  {name:<44} {:>5.1}% own octave  {:>5.1}% one out  {:>4.1}% further",
            share(self.own),
            share(self.one_out),
            share(self.further)
        )
    }
}

/// What's asked of one kind of part, and how each rule did.
#[derive(Default)]
struct Asked {
    stretches: usize,
    /// Beside the song's other kinds of part only, as if it had none of this kind.
    alone_usual: Count,
    alone_old: Count,
    /// Beside all the rest of the song, where it has another part of this kind.
    with_own: usize,
    own_usual: Count,
    own_beside: Count,
    own_half_way: Count,
    own_shipped: Count,
    own_old: Count,
}

pub fn run(args: &[String]) -> Result<(), String> {
    let mut paths = Vec::new();
    let (mut bars, mut folds) = (8i64, vec![1usize, 2, 3, 4]);
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
        match arg.as_str() {
            "--bars" => bars = value("--bars")?.parse().map_err(|_| "--bars takes a number".to_owned())?,
            "--folds" => {
                folds = value("--folds")?
                    .split(',')
                    .map(|f| f.trim().parse::<usize>().map_err(|_| format!("{f} is no fold")))
                    .collect::<Result<_, _>>()?;
            }
            other if other.starts_with("--") => return Err(format!("octave doesn't know {other}")),
            path => paths.push(PathBuf::from(path)),
        }
    }
    if paths.is_empty() || bars <= 0 {
        return Err("octave takes labelled files, and --bars more than 0".to_owned());
    }
    let mut items: Vec<Item> = Vec::new();
    for path in &paths {
        items.extend(item::read(path)?);
    }
    // The songs `pairs` takes, and its folds.
    let kept = items.iter().filter(|i| i.aligned && i.beats_per_bar == 4 && folds.contains(&keyfit::own_fold(i)));

    let mut songs = 0usize;
    let mut asked: Vec<(Role, Asked)> = [Role::Melody, Role::Chords, Role::Bass].map(|r| (r, Asked::default())).into();
    for item in kept {
        let parts = parts_of(item);
        if parts.len() < 2 {
            continue;
        }
        songs += 1;
        let length = bars * item.bar();
        for (at, part) in parts.iter().enumerate() {
            let Some((_, asked)) = asked.iter_mut().find(|(role, _)| *role == part.role) else { continue };
            let others: Vec<&Part> = parts.iter().enumerate().filter(|(o, _)| *o != at).map(|(_, p)| p).collect();
            let other_kinds: Vec<&Part> = others.iter().copied().filter(|p| p.role != part.role).collect();
            // Beside the rest of the song, and beside its other kinds of part only (which have no place for this
            // kind, so it goes by its usual one).
            let registers = Registers::of_parts(others.iter().map(|p| (p.role, p.notes.as_slice())));
            let alone = Registers::of_parts(other_kinds.iter().map(|p| (p.role, p.notes.as_slice())));
            let in_song = registers.of_kind(part.role);
            let Some(usual) = usual(part.role) else { continue };
            let least = if part.role == Role::Melody { LEAST_TUNE } else { LEAST_BACKING };
            for from in item.stretch_starts(bars) {
                let stretch = as_app(item, &item::cut(&part.labelled, from, length));
                if stretch.len() < least {
                    continue;
                }
                let (Some(middle), Some(centre)) = (middle(&stretch), old_centre(&stretch)) else { continue };
                let Some(edges) = edges(&stretch) else { continue };
                asked.stretches += 1;
                asked.alone_usual.add(alone.octaves_for(part.role, edges, 0));
                asked.alone_old.add(old_octaves(&other_kinds, part.role, centre));
                if let Some(in_song) = in_song {
                    asked.with_own += 1;
                    asked.own_usual.add(octaves_to(usual, middle));
                    asked.own_beside.add(octaves_to(in_song, middle));
                    asked.own_half_way.add(octaves_to((usual + in_song) / 2.0, middle));
                    asked.own_shipped.add(registers.octaves_for(part.role, edges, 0));
                    asked.own_old.add(old_octaves(&others, part.role, centre));
                }
            }
        }
    }

    let folds: Vec<String> = folds.iter().map(usize::to_string).collect();
    println!(
        "octave: {songs} songs (4/4, bar lines on the grid, folds {}), {bars} bars at a time: how often a part is put \
         back at the octave it was written at",
        folds.join(",")
    );
    for (role, asked) in asked.iter().filter(|(_, asked)| asked.stretches > 0) {
        println!();
        println!("{}: {} stretches, beside the song's other kinds of part only", role.plural(), asked.stretches);
        println!("{}", asked.alone_usual.line("the app: its kind's usual place"));
        println!("{}", asked.alone_old.line("the C# app: the usual place, nudged by them"));
        if asked.with_own > 0 {
            println!(
                "{}: {} stretches, beside a song that has another part of the kind",
                role.plural(),
                asked.with_own
            );
            let shipped = if stands_in(*role) { "half-way between the two" } else { "its kind's usual place" };
            println!("{}", asked.own_shipped.line(&format!("the app: {shipped}")));
            println!("{}", asked.own_usual.line("its kind's usual place"));
            println!("{}", asked.own_half_way.line("half-way between the two"));
            println!("{}", asked.own_beside.line("where the song's other one plays"));
            println!("{}", asked.own_old.line("the C# app: where the song's other one plays"));
        }
    }
    Ok(())
}
