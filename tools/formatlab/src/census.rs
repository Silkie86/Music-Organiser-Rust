//! Counts, across many projects, what's in them that the field registry doesn't know yet: FL Studio events, Bitwig
//! classes and fields. Ranked by how many projects use each, it says what's worth decoding next. It keeps only ids
//! and counts, nothing of the music.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use rayon::prelude::*;

use crate::bitwig_lab::class_label;
use crate::lab::count;
use crate::registry::{self, Registry};

/// How many files that couldn't be read are kept, with why.
const UNREADABLE_KEPT: usize = 20;

/// How often one id turned up.
#[derive(Clone, Debug, Default)]
pub struct Tally {
    pub projects: usize,
    pub times: u64,
    /// FL Studio: payload sizes. Bitwig: value types.
    pub shapes: HashMap<usize, usize>,
    /// Bitwig fields: the classes they were in.
    pub classes: HashMap<u32, usize>,
    pub versions: BTreeSet<String>,
}

impl Tally {
    fn merge(&mut self, o: Tally) {
        self.projects += o.projects;
        self.times += o.times;
        for (k, v) in o.shapes {
            *self.shapes.entry(k).or_default() += v;
        }
        for (k, v) in o.classes {
            *self.classes.entry(k).or_default() += v;
        }
        self.versions.extend(o.versions);
    }
}

#[derive(Clone, Debug, Default)]
pub struct Census {
    pub fl_projects: usize,
    pub bitwig_projects: usize,
    /// Bitwig projects that didn't decode to the end (what came before the problem is still counted).
    pub bitwig_partly: usize,
    pub fl_versions: BTreeMap<String, usize>,
    pub bitwig_versions: BTreeMap<String, usize>,
    pub fl_events: HashMap<u32, Tally>,
    pub bitwig_classes: HashMap<u32, Tally>,
    pub bitwig_fields: HashMap<u32, Tally>,
    /// Files that couldn't be read at all, with why (the first 20).
    pub unreadable: Vec<(PathBuf, String)>,
    pub unreadable_count: usize,
}

/// A project the census counts, by its ending.
pub fn is_project(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("flp") || e.eq_ignore_ascii_case("bwproject"))
}

/// Every project under `folders`, in name order, and the project files named straight out.
pub fn find_projects(paths: &[PathBuf]) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let Ok(kind) = e.file_type() else { continue };
            let path = e.path();
            // Links are left alone, so a link back up the tree can't send the walk round in circles.
            if kind.is_dir() {
                walk(&path, out);
            } else if kind.is_file() && is_project(&path) {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    for p in paths {
        if p.is_dir() {
            walk(p, &mut out);
        } else if p.is_file() && is_project(p) {
            out.push(p.clone());
        }
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|p| seen.insert(p.clone()));
    out
}

impl Census {
    /// Counts every .flp and .bwproject in `files`, several at a time, saying how many are done every 100.
    pub fn run(files: &[PathBuf], progress: impl Fn(usize) + Sync) -> Census {
        let done = AtomicUsize::new(0);
        let count = || {
            files
                .par_iter()
                .filter(|f| is_project(f))
                .fold(Census::default, |mut mine, file| {
                    mine.add(file);
                    let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                    if n.is_multiple_of(100) {
                        progress(n);
                    }
                    mine
                })
                .reduce(Census::default, |mut a, b| {
                    a.merge(b);
                    a
                })
        };
        // All cores but one, so the computer stays usable while it counts.
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get().saturating_sub(1).max(1));
        let census = match rayon::ThreadPoolBuilder::new().num_threads(threads).build() {
            Ok(pool) => pool.install(count),
            Err(_) => count(),
        };
        progress(done.load(Ordering::Relaxed));
        census
    }

    /// Counts one project.
    pub fn add(&mut self, path: &Path) {
        let result = std::fs::read(path).map_err(|e| e.to_string()).and_then(|raw| {
            if mo_formats::bitwig::is_bitwig(&raw) { self.add_bitwig(&raw) } else { self.add_fl(&raw) }
        });
        if let Err(why) = result {
            self.unreadable_count += 1;
            if self.unreadable.len() < UNREADABLE_KEPT {
                self.unreadable.push((path.to_owned(), why));
            }
        }
    }

    fn add_fl(&mut self, raw: &[u8]) -> Result<(), String> {
        let flp = mo_formats::fl::events::load(raw).map_err(|e| e.to_string())?;
        let version = if flp.major > 0 { flp.major.to_string() } else { "unknown".to_owned() };
        self.fl_projects += 1;
        *self.fl_versions.entry(version.clone()).or_default() += 1;
        let mut mine: HashMap<u32, Tally> = HashMap::new();
        // Only what's in the file, not the events the reader adds (an old FL's step sequencer dots read as notes).
        for e in flp.events.iter().filter(|e| e.offset.is_some()) {
            let t = mine.entry(u32::from(e.id)).or_insert_with(|| Tally { projects: 1, ..Tally::default() });
            t.times += 1;
            *t.shapes.entry(e.data.len()).or_default() += 1;
        }
        for (id, mut t) in mine {
            t.versions.insert(version.clone());
            self.fl_events.entry(id).or_default().merge(t);
        }
        Ok(())
    }

    fn add_bitwig(&mut self, raw: &[u8]) -> Result<(), String> {
        let doc = mo_formats::bitwig::decode(raw.to_vec()).map_err(|_| "This isn't a Bitwig file".to_owned())?;
        let v = doc.meta_str("application_version_name");
        let major_minor: Vec<&str> = v.split(['.', ' ']).take(2).collect();
        let version = match major_minor.join(".") {
            mm if mm.is_empty() => "unknown".to_owned(),
            mm => mm,
        };
        self.bitwig_projects += 1;
        if doc.body_error.is_some() {
            self.bitwig_partly += 1;
        }
        *self.bitwig_versions.entry(version.clone()).or_default() += 1;
        let mut classes: HashMap<u32, Tally> = HashMap::new();
        let mut fields: HashMap<u32, Tally> = HashMap::new();
        for o in &doc.objects {
            let c = classes.entry(o.class).or_insert_with(|| Tally { projects: 1, ..Tally::default() });
            c.times += 1;
            for field in &o.fields {
                let f = fields.entry(field.id).or_insert_with(|| Tally { projects: 1, ..Tally::default() });
                f.times += 1;
                *f.shapes.entry(usize::from(field.value.type_byte())).or_default() += 1;
                *f.classes.entry(o.class).or_default() += 1;
            }
        }
        for (id, mut t) in classes {
            t.versions.insert(version.clone());
            self.bitwig_classes.entry(id).or_default().merge(t);
        }
        for (id, mut t) in fields {
            t.versions.insert(version.clone());
            self.bitwig_fields.entry(id).or_default().merge(t);
        }
        Ok(())
    }

    fn merge(&mut self, o: Census) {
        self.fl_projects += o.fl_projects;
        self.bitwig_projects += o.bitwig_projects;
        self.bitwig_partly += o.bitwig_partly;
        self.unreadable_count += o.unreadable_count;
        for (k, v) in o.fl_versions {
            *self.fl_versions.entry(k).or_default() += v;
        }
        for (k, v) in o.bitwig_versions {
            *self.bitwig_versions.entry(k).or_default() += v;
        }
        for (into, from) in [
            (&mut self.fl_events, o.fl_events),
            (&mut self.bitwig_classes, o.bitwig_classes),
            (&mut self.bitwig_fields, o.bitwig_fields),
        ] {
            for (k, t) in from {
                into.entry(k).or_default().merge(t);
            }
        }
        let room = UNREADABLE_KEPT.saturating_sub(self.unreadable.len());
        self.unreadable.extend(o.unreadable.into_iter().take(room));
    }

    /// FL events the registry doesn't have, most used first.
    pub fn unknown_fl_events(&self) -> Vec<(u32, &Tally)> {
        unknown(&self.fl_events, registry::fl_studio(), Registry::event)
    }

    pub fn unknown_bitwig_classes(&self) -> Vec<(u32, &Tally)> {
        unknown(&self.bitwig_classes, registry::bitwig(), Registry::class)
    }

    pub fn unknown_bitwig_fields(&self) -> Vec<(u32, &Tally)> {
        unknown(&self.bitwig_fields, registry::bitwig(), Registry::field)
    }

    /// The census as text: names and counts only, to paste into a conversation.
    pub fn report(&self, top: usize) -> String {
        let mut out = String::new();
        let files = self.fl_projects + self.bitwig_projects + self.unreadable_count;
        let _ = writeln!(out, "Format census of {} project{}", count(files), plural(files));
        if self.unreadable_count > 0 {
            let _ = writeln!(out, "{} couldn't be read at all, for example:", count(self.unreadable_count));
            for (file, why) in self.unreadable.iter().take(5) {
                let name =
                    file.file_name().map_or_else(|| file.display().to_string(), |n| n.to_string_lossy().into_owned());
                let _ = writeln!(out, "  {name}: {why}");
            }
        }

        if self.fl_projects > 0 {
            let _ = writeln!(out);
            let _ = writeln!(
                out,
                "FL Studio: {} project{} ({})",
                count(self.fl_projects),
                plural(self.fl_projects),
                versions(&self.fl_versions, "FL ")
            );
            let unknown = self.unknown_fl_events();
            let _ = writeln!(
                out,
                "{} kinds of event; the registry knows {}. The rest, by how many projects have them:",
                count(self.fl_events.len()),
                count(self.fl_events.len() - unknown.len())
            );
            for (id, t) in unknown.iter().take(top) {
                let _ = writeln!(
                    out,
                    "  event {id:<4} {:<22} FL {:<9} {}",
                    share(t.projects, self.fl_projects),
                    span(&t.versions),
                    sizes(&t.shapes)
                );
            }
            more(&mut out, unknown.len(), top);
        }

        if self.bitwig_projects > 0 {
            let _ = writeln!(out);
            let partly = if self.bitwig_partly > 0 {
                format!("; {} only partly decoded", count(self.bitwig_partly))
            } else {
                String::new()
            };
            let _ = writeln!(
                out,
                "Bitwig Studio: {} project{} ({}){partly}",
                count(self.bitwig_projects),
                plural(self.bitwig_projects),
                versions(&self.bitwig_versions, "")
            );
            let classes = self.unknown_bitwig_classes();
            let _ = writeln!(
                out,
                "{} classes; the registry knows {}. The rest, by how many projects have them:",
                count(self.bitwig_classes.len()),
                count(self.bitwig_classes.len() - classes.len())
            );
            for (id, t) in classes.iter().take(top) {
                let times = usize::try_from(t.times).unwrap_or(usize::MAX);
                let _ = writeln!(
                    out,
                    "  class {:<8} {:<22} {} object{}",
                    format!("0x{id:x}"),
                    share(t.projects, self.bitwig_projects),
                    count(times),
                    plural(times)
                );
            }
            more(&mut out, classes.len(), top);
            let fields = self.unknown_bitwig_fields();
            let _ = writeln!(
                out,
                "{} fields; the registry knows {}. The rest, by how many projects have them:",
                count(self.bitwig_fields.len()),
                count(self.bitwig_fields.len() - fields.len())
            );
            for (id, t) in fields.iter().take(top) {
                let _ = writeln!(
                    out,
                    "  field {:<8} {:<22} {:<8} mostly in {}",
                    format!("0x{id:x}"),
                    share(t.projects, self.bitwig_projects),
                    types(&t.shapes),
                    mostly_in(&t.classes)
                );
            }
            more(&mut out, fields.len(), top);
        }
        out
    }
}

fn unknown<'a>(
    tallies: &'a HashMap<u32, Tally>,
    registry: &Registry,
    known: impl Fn(&Registry, u32) -> Option<&crate::registry::Entry>,
) -> Vec<(u32, &'a Tally)> {
    let mut list: Vec<(u32, &Tally)> =
        tallies.iter().filter(|(id, _)| known(registry, **id).is_none()).map(|(&id, t)| (id, t)).collect();
    list.sort_by(|a, b| b.1.projects.cmp(&a.1.projects).then(a.0.cmp(&b.0)));
    list
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

fn more(out: &mut String, n: usize, top: usize) {
    if n > top {
        let _ = writeln!(out, "  … and {} more", count(n - top));
    }
}

/// The versions with how many projects each, in version order ("FL 9: 1, FL 21: 3").
fn versions(v: &BTreeMap<String, usize>, prefix: &str) -> String {
    let order = |k: &str| -> Vec<u64> {
        let parts: Option<Vec<u64>> = k.split('.').map(|p| p.parse().ok()).collect();
        parts.unwrap_or_default()
    };
    let mut list: Vec<(&String, &usize)> = v.iter().collect();
    list.sort_by_key(|(k, _)| order(k));
    list.iter().map(|(k, n)| format!("{prefix}{k}: {}", count(**n))).collect::<Vec<_>>().join(", ")
}

/// "in 1 (33.3%)": how many of the projects, and what share.
fn share(n: usize, of: usize) -> String {
    let pct = 100.0 * n as f64 / of.max(1) as f64;
    let rounded = (pct * 10.0).round() / 10.0;
    format!("in {} ({}%)", count(n), rounded)
}

/// The lowest and highest FL version an event was seen in ("21-25"), or "?".
fn span(versions: &BTreeSet<String>) -> String {
    let mut numbers: Vec<u32> = versions.iter().filter_map(|v| v.parse().ok()).collect();
    numbers.sort_unstable();
    match (numbers.first(), numbers.last()) {
        (Some(a), Some(b)) if a == b => a.to_string(),
        (Some(a), Some(b)) => format!("{a}-{b}"),
        _ => "?".into(),
    }
}

/// The commonest payload sizes.
fn sizes(sizes: &HashMap<usize, usize>) -> String {
    let mut list: Vec<(usize, usize)> = sizes.iter().map(|(&k, &v)| (k, v)).collect();
    list.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let common: Vec<String> = list
        .iter()
        .take(3)
        .map(|&(k, _)| if k == 1 { "1 byte".into() } else { format!("{} bytes", count(k)) })
        .collect();
    if sizes.len() <= 3 { common.join(" or ") } else { format!("{} or others", common.join(", ")) }
}

/// The two commonest value types.
fn types(types: &HashMap<usize, usize>) -> String {
    let mut list: Vec<(usize, usize)> = types.iter().map(|(&k, &v)| (k, v)).collect();
    list.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    list.iter().take(2).map(|&(k, _)| type_name(k as u8)).collect::<Vec<_>>().join("/")
}

/// Bitwig's value types.
pub fn type_name(t: u8) -> String {
    match t {
        0x01 => "u8",
        0x02 => "i16",
        0x03 => "i32",
        0x04 => "i64",
        0x05 => "bool",
        0x06 => "f32",
        0x07 => "f64",
        0x08 => "string",
        0x09 => "object",
        0x0a => "null",
        0x0b => "ref",
        0x0d => "blob",
        0x0f => "ints",
        0x12 => "list",
        0x14 => "map",
        0x15 => "uuid",
        0x16 => "rgba",
        0x17 => "floats",
        0x18 => "doubles",
        0x19 => "strings",
        0x1a => "named",
        _ => return format!("0x{t:02x}"),
    }
    .to_owned()
}

/// The class a field was most often in.
fn mostly_in(classes: &HashMap<u32, usize>) -> String {
    classes.iter().max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0))).map_or_else(|| "?".into(), |(&id, _)| class_label(id))
}
