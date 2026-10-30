//! Format Lab's three looks at files: what's inside one ([`tree`]), every value that differs between two saves
//! ([`diff`]), and everywhere one holds a value ([`find`]), so a change made in the DAW can be traced to the bytes that
//! hold it.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use crate::lab::{self, Data, Format, Item, LabFile};

// ---- Tree ----

/// Everything in the file, one line each, indented by depth, down to `max_depth`.
pub fn tree(f: &LabFile, max_depth: usize) -> String {
    let mut out = String::new();
    heading(&mut out, f);
    let mut scope = None;
    for i in &f.items {
        if f.format == Format::FlStudio {
            // FL's parts come back now and then (a pattern's events in two places), so name the part each time it
            // changes.
            if i.depth == 0 {
                continue;
            }
            let s = i.key.split('/').next().unwrap_or_default();
            if scope != Some(s) {
                out.push_str(s);
                out.push('\n');
                scope = Some(s);
            }
        }
        if i.depth > max_depth {
            continue;
        }
        out.push_str(&" ".repeat(i.depth * 2));
        out.push_str(&i.label);
        if !i.value.is_empty() {
            out.push_str(if i.is_group { ": " } else { " = " });
            out.push_str(&i.value);
        }
        out.push('\n');
    }
    out
}

fn heading(out: &mut String, f: &LabFile) {
    let _ = writeln!(out, "{}", format!("{} {} · {}", f.daw(), f.version, f.name).replace("  ", " "));
    if let Some(p) = &f.problem {
        let _ = writeln!(out, "Only part of it could be read: {p}");
    }
    if let Some(w) = &f.warning {
        let _ = writeln!(out, "Warning: {w}");
    }
}

// ---- Diff ----

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    Changed,
    Added,
    Removed,
}

/// One difference between two saves of a project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub kind: ChangeKind,
    pub key: String,
    pub path: String,
    pub before: Option<String>,
    pub after: Option<String>,
    /// For something added or removed: how many things it holds, which aren't listed one by one.
    pub inside: usize,
    /// For bytes: where they differ.
    pub detail: Option<String>,
}

/// What differs between two saves: values that changed, and things added or removed (each listed once, with how much
/// it holds). A group (an object, a list, an event made of parts) is only listed as changed when nothing inside it is.
pub fn diff(before: &LabFile, after: &LabFile) -> Vec<Change> {
    let a = index(&before.items);
    let b = index(&after.items);
    let mut changed: Vec<(usize, usize, Change)> = Vec::new();
    let mut touched: HashSet<&str> = HashSet::new();

    for (order, y) in after.items.iter().enumerate() {
        if b.get(y.key.as_str()) != Some(&order) {
            continue;
        }
        if let Some(&xi) = a.get(y.key.as_str())
            && let Some(x) = before.items.get(xi)
            && !same(x, y)
        {
            changed.push((
                order,
                0,
                Change {
                    kind: ChangeKind::Changed,
                    key: y.key.clone(),
                    path: y.path.clone(),
                    before: Some(x.value.clone()),
                    after: Some(y.value.clone()),
                    inside: 0,
                    detail: bytes_detail(&x.data, &y.data),
                },
            ));
            touched.insert(&y.key);
        }
    }
    let added: HashSet<&str> = b.keys().filter(|k| !a.contains_key(*k)).copied().collect();
    let removed: HashSet<&str> = a.keys().filter(|k| !b.contains_key(*k)).copied().collect();
    touched.extend(&added);
    touched.extend(&removed);

    for (key, inside) in outermost(&added) {
        let Some(y) = b.get(key).and_then(|&i| after.items.get(i).map(|y| (i, y))) else { continue };
        let (order, y) = y;
        changed.push((
            order,
            1,
            Change {
                kind: ChangeKind::Added,
                key: key.to_owned(),
                path: y.path.clone(),
                before: None,
                after: Some(y.value.clone()),
                inside,
                detail: None,
            },
        ));
    }
    for (key, inside) in outermost(&removed) {
        let Some((order, x)) = a.get(key).and_then(|&i| before.items.get(i).map(|x| (i, x))) else { continue };
        changed.push((
            nearest_order(key, &b, order),
            2 + order,
            Change {
                kind: ChangeKind::Removed,
                key: key.to_owned(),
                path: x.path.clone(),
                before: Some(x.value.clone()),
                after: None,
                inside,
                detail: None,
            },
        ));
    }
    // A group whose own text changed because something inside it did ("list of 3" to "list of 4") isn't news.
    let mut holders = HashSet::new();
    for t in &touched {
        for (i, _) in t.match_indices('/') {
            if i > 0 {
                holders.insert(t.get(..i).unwrap_or_default());
            }
        }
    }
    changed.retain(|(_, _, c)| !(c.kind == ChangeKind::Changed && holders.contains(c.key.as_str())));
    changed.sort_by_key(|&(order, then, _)| (order, then));
    changed.into_iter().map(|(_, _, c)| c).collect()
}

/// Each key's first place in the items.
fn index(items: &[Item]) -> HashMap<&str, usize> {
    let mut d = HashMap::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        d.entry(item.key.as_str()).or_insert(i);
    }
    d
}

/// The keys not inside another of them, each with how many of the others are inside it.
fn outermost<'a>(keys: &HashSet<&'a str>) -> Vec<(&'a str, usize)> {
    let mut top: HashMap<&str, usize> = HashMap::new();
    let mut inner = Vec::new();
    for &k in keys {
        match holder(k, keys) {
            None => {
                top.insert(k, 0);
            }
            Some(h) => inner.push(h),
        }
    }
    for h in inner {
        if let Some(n) = top.get_mut(h) {
            *n += 1;
        }
    }
    top.into_iter().collect()
}

/// The outermost of `keys` that holds `key`, if any.
fn holder<'a>(key: &'a str, keys: &HashSet<&str>) -> Option<&'a str> {
    key.match_indices('/')
        .filter(|&(i, _)| i > 0)
        .map(|(i, _)| key.get(..i).unwrap_or_default())
        .find(|p| keys.contains(p))
}

/// Where a removed thing would have been in the newer file: after what came before it.
fn nearest_order(key: &str, b: &HashMap<&str, usize>, fallback: usize) -> usize {
    key.rmatch_indices('/')
        .filter(|&(i, _)| i > 0)
        .find_map(|(i, _)| b.get(key.get(..i).unwrap_or_default()).copied())
        .unwrap_or(fallback)
}

fn same(x: &Item, y: &Item) -> bool {
    x.value == y.value
        && match (&x.data, &y.data) {
            (Data::Bytes(p), Data::Bytes(q)) => p == q,
            _ => true,
        }
}

fn bytes_detail(x: &Data, y: &Data) -> Option<String> {
    let (Data::Bytes(p), Data::Bytes(q)) = (x, y) else { return None };
    if p.len() != q.len() {
        let common = p.iter().zip(q).take_while(|(a, b)| a == b).count();
        return Some(format!(
            "{} → {} bytes; the same for the first {}",
            lab::count(p.len()),
            lab::count(q.len()),
            lab::count(common)
        ));
    }
    let differ: Vec<usize> = p.iter().zip(q).enumerate().filter(|(_, (a, b))| a != b).map(|(i, _)| i).collect();
    let (&first, &last) = (differ.first()?, differ.last()?);
    let to = (last + 1).max(first + 4).min((first + 16).min(p.len()));
    let place = if differ.len() == 1 {
        format!("byte {first}")
    } else {
        format!("{} bytes between {first} and {last}", lab::count(differ.len()))
    };
    Some(format!(
        "{place}: {} → {}",
        lab::hex(p.get(first..to).unwrap_or_default()),
        lab::hex(q.get(first..to).unwrap_or_default())
    ))
}

pub fn diff_text(before: &LabFile, after: &LabFile, changes: &[Change]) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{} {} → {} · {} → {}", before.daw(), before.version, after.version, before.name, after.name);
    for f in [before, after] {
        if let Some(p) = &f.problem {
            let _ = writeln!(out, "Only part of {} could be read: {p}", f.name);
        }
        if let Some(w) = &f.warning {
            let _ = writeln!(out, "Warning about {}: {w}", f.name);
        }
    }
    let _ = match changes.len() {
        0 => writeln!(out, "No differences."),
        1 => writeln!(out, "1 difference:"),
        n => writeln!(out, "{} differences:", lab::count(n)),
    };
    for c in changes {
        let inside = match c.inside {
            0 => String::new(),
            1 => " (with 1 thing inside)".to_owned(),
            n => format!(" (with {} things inside)", lab::count(n)),
        };
        let shown =
            |v: &Option<String>| v.as_deref().filter(|v| !v.is_empty()).map(|v| format!(": {v}")).unwrap_or_default();
        let _ = match c.kind {
            ChangeKind::Changed => {
                let _ = writeln!(out, "changed  {}", c.path);
                let _ = writeln!(
                    out,
                    "         {} → {}",
                    c.before.as_deref().unwrap_or_default(),
                    c.after.as_deref().unwrap_or_default()
                );
                match &c.detail {
                    Some(d) => writeln!(out, "         {d}"),
                    None => Ok(()),
                }
            }
            ChangeKind::Added => writeln!(out, "added    {}{}{inside}", c.path, shown(&c.after)),
            ChangeKind::Removed => writeln!(out, "removed  {}{}{inside}", c.path, shown(&c.before)),
        };
    }
    out
}

// ---- Find ----

/// A place in a file that holds what was looked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub path: String,
    pub value: String,
    /// How it was found: as text, as a number, or as bytes at an offset in an item's data.
    pub how: String,
}

/// A find: which item, the hit, and for a find in its bytes, where in them and how many.
type Found = (usize, Hit, Option<(usize, usize)>);

/// How many times one pattern is listed in one item's data.
const TIMES: usize = 10;

/// Everywhere the file holds `query`: as text (ignoring case), and, when it's a number, as a number in any field, or in
/// any data as a 32-bit integer, a 32- or 64-bit float (either byte order). Text is also looked for inside data, as
/// UTF-8 and UTF-16. Bytes that a part of the data found on its own (a record, or a number in it) aren't listed again
/// for the data around it.
pub fn find(f: &LabFile, query: &str, text_only: bool) -> Vec<Hit> {
    if query.is_empty() {
        return Vec::new();
    }
    let number = if text_only { None } else { parse_number(query) };
    let needle = query.to_lowercase();
    let has = |s: &str| s.to_lowercase().contains(&needle);
    let patterns = patterns(query, number);
    let items = &f.items;
    let mut found: Vec<Found> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let hit = |how: String| Hit { path: item.path.clone(), value: item.value.clone(), how };
        match &item.data {
            Data::Text(s) if has(s) => found.push((i, hit("text".into()), None)),
            Data::Int(l) if number.is_some_and(|x| *l as f64 == x) => found.push((i, hit("number".into()), None)),
            Data::Float(d) if number.is_some_and(|x| close(*d, x)) => found.push((i, hit("number".into()), None)),
            Data::Ints(a) => {
                if let Some(k) = number.and_then(|x| a.iter().position(|&v| v as f64 == x)) {
                    found.push((i, hit(format!("number, item {k} of the list")), None));
                }
            }
            Data::Floats(a) => {
                if let Some(k) = number.and_then(|x| a.iter().position(|&v| close(v, x))) {
                    found.push((i, hit(format!("number, item {k} of the list")), None));
                }
            }
            Data::Texts(a) => {
                if let Some(k) = a.iter().position(|v| has(v)) {
                    found.push((i, hit(format!("text, item {k} of the list")), None));
                }
            }
            Data::Bytes(b) => {
                for (name, bytes) in &patterns {
                    let starts = b.windows(bytes.len()).enumerate().filter(|(_, w)| w == bytes).map(|(at, _)| at);
                    for at in starts.take(TIMES) {
                        found.push((i, hit(format!("{name} at byte {}", lab::count(at))), Some((at, bytes.len()))));
                    }
                }
            }
            _ => {}
        }
    }

    // Each item's parent, and which items hold a find (in themselves or deeper down).
    let mut parent = vec![None; items.len()];
    let mut open: Vec<usize> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        while open.last().is_some_and(|&o| items.get(o).is_some_and(|x| x.depth >= item.depth)) {
            open.pop();
        }
        if let Some(&o) = open.last()
            && items.get(o).is_some_and(|x| x.depth + 1 == item.depth)
        {
            parent[i] = Some(o);
        }
        open.push(i);
    }
    let mut holds = vec![false; items.len()];
    for &(i, _, _) in &found {
        let mut p = Some(i);
        while let Some(at) = p {
            if holds[at] {
                break;
            }
            holds[at] = true;
            p = parent[at];
        }
    }
    // The parts of an item, with where they sit in its data, that hold a find.
    let mut children: HashMap<usize, Vec<(usize, usize)>> = HashMap::new();
    for (i, item) in items.iter().enumerate() {
        if let (Some(p), true, Some(span)) = (parent[i], holds[i], item.span) {
            children.entry(p).or_default().push(span);
        }
    }
    let covered = |item: usize, at: usize, len: usize| {
        children.get(&item).is_some_and(|kids| kids.iter().any(|&(start, n)| start < at + len && at < start + n))
    };
    found.into_iter().filter(|(i, _, at)| at.is_none_or(|(at, len)| !covered(*i, at, len))).map(|(_, h, _)| h).collect()
}

/// A query that's a number, as .NET reads one: digits with a sign, a point and an exponent, spaces around it allowed.
fn parse_number(query: &str) -> Option<f64> {
    let q = query.trim();
    let numeric =
        !q.is_empty() && q.bytes().all(|b| b.is_ascii_digit() || matches!(b, b'+' | b'-' | b'.' | b'e' | b'E'));
    numeric.then(|| q.parse().ok()).flatten()
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9_f64.max(b.abs() * 1e-6)
}

fn patterns(query: &str, number: Option<f64>) -> Vec<(&'static str, Vec<u8>)> {
    let mut p = Vec::new();
    match number {
        Some(n) => {
            if n == n.floor() && n >= f64::from(i32::MIN) && n <= f64::from(u32::MAX) && n.abs() >= 2.0 {
                let b = if n <= f64::from(i32::MAX) { (n as i32).to_le_bytes() } else { (n as u32).to_le_bytes() };
                p.push(("32-bit integer", b.to_vec()));
                p.push(("32-bit integer, big-endian", b.iter().rev().copied().collect()));
            }
            let f = (n as f32).to_le_bytes();
            p.push(("32-bit float", f.to_vec()));
            p.push(("32-bit float, big-endian", f.iter().rev().copied().collect()));
            let d = n.to_le_bytes();
            p.push(("64-bit float", d.to_vec()));
            p.push(("64-bit float, big-endian", d.iter().rev().copied().collect()));
        }
        None => {
            p.push(("UTF-8 text", query.as_bytes().to_vec()));
            p.push(("UTF-16 text", query.encode_utf16().flat_map(u16::to_le_bytes).collect()));
            p.push(("UTF-16 text, big-endian", query.encode_utf16().flat_map(u16::to_be_bytes).collect()));
        }
    }
    p
}

pub fn find_text(f: &LabFile, query: &str, hits: &[Hit], max: usize) -> String {
    let mut out = String::new();
    heading(&mut out, f);
    let q = lab::quote(query);
    let _ = match hits.len() {
        0 => writeln!(out, "{q} isn't in it."),
        1 => writeln!(out, "Found {q} once:"),
        n => writeln!(out, "Found {q} {} times:", lab::count(n)),
    };
    for h in hits.iter().take(max) {
        let _ = writeln!(out, "  {} = {}", h.path, h.value);
        let _ = writeln!(out, "    as {}", h.how);
    }
    if hits.len() > max {
        let _ = writeln!(out, "  … and {} more", lab::count(hits.len() - max));
    }
    out
}
