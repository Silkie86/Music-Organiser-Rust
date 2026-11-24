//! The Sounds page's folder tree (Solomon, 24 Nov 2026): every folder that holds a sound, as a tree that starts at
//! your sample folders, each line with how many sounds the list would show from it with the other filters as they
//! are. You pick as many folders as you like (each with the folders inside it, or just that folder), and hide the
//! ones you never want to see.
//!
//! [`Folders`] is worked out once per list of sounds ([`crate::sounds::SoundList`]): which folder each sound is in,
//! and the folders above them. [`rows`] makes the lines the tree shows from it, the counts of the newest list worked
//! out, your picks, the branches you opened and the words in its search box. Nothing here is stored but your picks,
//! your hidden folders and the branches you opened (as settings, [`encode_picks`]).

use std::collections::{HashMap, HashSet};

use mo_formats::case;

use crate::sounds::{by_letters, short_count};

/// What joins the folders of a line that stands for a chain of folders that each hold only the next one.
pub const CHAIN: &str = " › ";

/// A folder picked on the tree: its path as the tree has it, and whether only the sounds right in it count (not
/// those in the folders inside it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderPick {
    pub path: String,
    pub just_this: bool,
}

/// One folder of the tree.
#[derive(Debug)]
struct Node {
    /// Its own name, as it's spelt in the first path met with it ("Kicks"; "I:\\" for a drive's top).
    name: String,
    /// Its whole path, as spelt in the first path met with it.
    path: String,
    /// The folder it's in; none for a drive's or disk's top.
    parent: Option<u32>,
    /// How many of the list's sounds are right in it.
    sounds: u32,
}

/// Which folder each of a list's sounds is in, and every folder above them: the folder tree's bones. The folders are
/// told apart as the app tells files apart: capitals aside, and `/` taken for `\`. A folder always comes after the
/// folder it's in.
#[derive(Debug, Default)]
pub struct Folders {
    nodes: Vec<Node>,
    /// The folder each sound is in, by its place in the list; [`NONE`] for a bare name.
    of_sound: Vec<u32>,
    by_key: HashMap<String, u32>,
    /// The folders right in each folder.
    children: Vec<Vec<u32>>,
    /// Which making of the folders this is: counts made for another ([`FolderCounts::of`]) aren't taken for these.
    stamp: u64,
}

/// Counts the makings of [`Folders`], for their stamps.
static MADE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// The folder of a sound that has none (a bare name, which a library never holds).
pub const NONE: u32 = u32::MAX;

/// How a folder's path is known whichever way it's written: capitals aside, `/` taken for `\`, no `\` at its end
/// (but a drive's top keeps its own).
pub fn key(path: &str) -> String {
    let path = path.trim_end_matches(['\\', '/']);
    let path = if path.is_empty() { "\\" } else { path };
    path.chars().map(|c| if c == '/' { '\\' } else { case::upper(c) }).collect()
}

/// Where each folder of `dir` ends: its path up to each `\` or `/`, then the whole of it. A run of separators at the
/// start (a Mac or Linux disk's top, a network share's `\\`) is its top folder.
fn ends(dir: &str) -> Vec<usize> {
    let lead = dir.len() - dir.trim_start_matches(['\\', '/']).len();
    let mut out = Vec::new();
    if lead > 0 {
        out.push(lead);
    }
    for (i, c) in dir.char_indices().skip(lead) {
        if c == '\\' || c == '/' {
            out.push(i);
        }
    }
    if out.last() != Some(&dir.len()) {
        out.push(dir.len());
    }
    out
}

/// The folder `path` is in: all of it before its last `\` or `/`; none for a bare name.
fn dir_of(path: &str) -> Option<&str> {
    let i = path.rfind(['\\', '/'])?;
    Some(if i == 0 { &path[..1] } else { &path[..i] })
}

impl Folders {
    /// The folders of the sounds at `paths`, in the list's order.
    pub fn new<'a>(paths: impl Iterator<Item = &'a str>) -> Folders {
        let mut f = Folders { stamp: MADE.fetch_add(1, std::sync::atomic::Ordering::Relaxed), ..Folders::default() };
        // Each spelling of a folder looked up once, not once a sound.
        let mut seen: HashMap<&str, u32> = HashMap::new();
        for path in paths {
            let at = match dir_of(path) {
                None => NONE,
                Some(dir) => match seen.get(dir) {
                    Some(&at) => at,
                    None => {
                        let at = f.add(dir);
                        seen.insert(dir, at);
                        at
                    }
                },
            };
            if let Some(node) = f.nodes.get_mut(at as usize) {
                node.sounds += 1;
            }
            f.of_sound.push(at);
        }
        f.children = vec![Vec::new(); f.nodes.len()];
        for (i, n) in f.nodes.iter().enumerate() {
            if let Some(p) = n.parent {
                f.children[p as usize].push(i as u32);
            }
        }
        f.of_sound.shrink_to_fit();
        f
    }

    /// The folder at `dir`, and the folders above it, made where they aren't yet.
    fn add(&mut self, dir: &str) -> u32 {
        let mut parent = None;
        let mut start = 0;
        for end in ends(dir) {
            let path = &dir[..end];
            let k = key(path);
            let at = match self.by_key.get(&k) {
                Some(&at) => at,
                None => {
                    let own = dir[start..end].trim_start_matches(['\\', '/']);
                    let top = parent.is_none();
                    let is_drive = matches!(path.as_bytes(), [l, b':'] if l.is_ascii_alphabetic());
                    let name = match (top, own.is_empty()) {
                        (true, true) => path.to_owned(),
                        (true, false) if is_drive => format!("{path}\\"),
                        _ => own.to_owned(),
                    };
                    let shown = if is_drive { format!("{path}\\") } else { path.to_owned() };
                    self.nodes.push(Node { name, path: shown, parent, sounds: 0 });
                    let at = (self.nodes.len() - 1) as u32;
                    self.by_key.insert(k, at);
                    at
                }
            };
            parent = Some(at);
            start = end;
        }
        parent.unwrap_or(NONE)
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Which making of the folders this is ([`FolderCounts::of`]).
    pub fn stamp(&self) -> u64 {
        self.stamp
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// The folder the sound at place `i` is in ([`NONE`] for none).
    pub fn of_sound(&self, i: usize) -> u32 {
        self.of_sound.get(i).copied().unwrap_or(NONE)
    }

    /// The folder at `path`, if a sound is in it or in a folder inside it.
    pub fn find(&self, path: &str) -> Option<u32> {
        self.by_key.get(&key(path)).copied()
    }

    /// Every folder inside folder `n`, all the way down.
    pub fn inside(&self, n: u32) -> Vec<u32> {
        let mut out = Vec::new();
        let mut next = vec![n];
        while let Some(at) = next.pop() {
            for &c in self.children.get(at as usize).map(Vec::as_slice).unwrap_or_default() {
                out.push(c);
                next.push(c);
            }
        }
        out
    }

    /// The whole path of folder `n`.
    pub fn path(&self, n: u32) -> &str {
        &self.nodes[n as usize].path
    }

    /// Which folders' sounds the picks and the hidden folders let through, by folder, and whether a sound in no folder
    /// is let through: none when nothing is picked or hidden. A folder picked with the folders inside it lets them
    /// through too; a hidden folder keeps out the folders inside it too, even where one of them is picked. A pick of
    /// a folder no sound is in lets nothing through, so with only such picks the list is empty: it says what you
    /// picked, not every sound. A sample folder inside another (`tops`) is a top line of its own on the tree, so a
    /// pick or a hide of the folder above it doesn't reach it, as the tree shows.
    pub fn mask(&self, picks: &[FolderPick], hidden: &[String], tops: &[String]) -> Option<FolderMask> {
        if picks.is_empty() && hidden.is_empty() {
            return None;
        }
        let n = self.nodes.len();
        let mut with_inside = vec![false; n];
        let mut just = vec![false; n];
        for p in picks {
            if let Some(at) = self.find(&p.path) {
                if p.just_this {
                    just[at as usize] = true;
                } else {
                    with_inside[at as usize] = true;
                }
            }
        }
        let mut gone = vec![false; n];
        for h in hidden {
            if let Some(at) = self.find(h) {
                gone[at as usize] = true;
            }
        }
        let mut is_top = vec![false; n];
        for t in tops {
            if let Some(at) = self.find(t) {
                is_top[at as usize] = true;
            }
        }
        let mut ok = vec![false; n];
        // A folder comes after the folder it's in, so its parent's answers are known by its turn.
        for i in 0..n {
            if let Some(p) = self.nodes[i].parent.filter(|_| !is_top[i]) {
                with_inside[i] |= with_inside[p as usize];
                gone[i] |= gone[p as usize];
            }
            ok[i] = !gone[i] && (picks.is_empty() || with_inside[i] || just[i]);
        }
        Some(FolderMask { ok, none_ok: picks.is_empty() })
    }
}

/// Which folders' sounds the folder tree lets through ([`Folders::mask`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderMask {
    ok: Vec<bool>,
    none_ok: bool,
}

impl FolderMask {
    /// Whether a sound in folder `n` is let through.
    pub fn allows(&self, n: u32) -> bool {
        if n == NONE { self.none_ok } else { self.ok.get(n as usize).copied().unwrap_or(false) }
    }
}

/// How a line of the tree is picked.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Picked {
    #[default]
    No,
    /// Picked, with the folders inside it.
    Yes,
    /// Picked, only the sounds right in it.
    JustThis,
    /// Inside a folder picked with the folders inside it: its sounds show, though it isn't picked itself.
    Within,
    /// A folder inside it is picked.
    Part,
}

/// One line of the folder tree, as the page shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FolderRow {
    /// The path the line picks: the last folder of its chain.
    pub path: String,
    /// What it says: the folder's name, a chain's names joined by [`CHAIN`], a top line's whole path.
    pub label: String,
    /// How far in it is (0 for a top line).
    pub depth: usize,
    /// How many sounds it would show with the other filters as they are ("1.2k"); "" before they're counted.
    pub count: String,
    /// How many of those your projects use ("12 used"); "" until the use is known.
    pub used: String,
    /// It has folders inside it to show.
    pub has_children: bool,
    pub open: bool,
    pub picked: Picked,
    /// You hid it.
    pub hidden: bool,
    /// Its drive isn't there just now (a top line).
    pub away: bool,
    /// No sound in it is let through by the other filters: it shows only as it's picked, hidden or holds a folder
    /// that is.
    pub empty: bool,
}

/// How many sounds are right in each folder, of those all filters but the folder tree let through, and how many of
/// them your projects use ([`crate::sounds::SoundCounts::folders`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FolderCounts {
    /// The stamp of the folders they were counted by ([`Folders::stamp`]): counts of other folders are taken for none.
    pub of: u64,
    pub sounds: Vec<u32>,
    /// None until the use is known.
    pub used: Option<Vec<u32>>,
}

/// What the tree shows besides the folders: your picks, hidden folders, the branches open, the search box's words,
/// your sample folders (the tops it starts at) and the drives away.
#[derive(Clone, Copy)]
pub struct TreeAsk<'a> {
    pub picks: &'a [FolderPick],
    pub hidden: &'a [String],
    /// The folders opened, by [`key`].
    pub open: &'a HashSet<String>,
    pub search: &'a str,
    pub tops: &'a [String],
    /// The tops whose drive isn't there, by [`key`].
    pub away: &'a HashSet<String>,
}

/// The tree's lines, in the order they show: each top line, then the open folders inside it, by name. Only folders
/// with sounds the other filters let through show (with the folders above them), and those picked, hidden or holding
/// a folder that is, so a pick can always be taken off. With words in the search box, only folders whose line has
/// them all show, with the folders above them opened. `counts` are none until the newest list is counted: every
/// folder shows, with no numbers.
pub fn rows(f: &Folders, counts: Option<&FolderCounts>, ask: TreeAsk) -> Vec<FolderRow> {
    let counts = counts.filter(|c| c.of == f.stamp && c.sounds.len() == f.nodes.len());
    let n = f.nodes.len();
    if n == 0 {
        return Vec::new();
    }
    // Each sample folder found once, not each folder's path worked out again.
    let mut is_top: Vec<bool> = f.nodes.iter().map(|n| n.parent.is_none()).collect();
    for t in ask.tops {
        if let Some(at) = f.find(t) {
            is_top[at as usize] = true;
        }
    }
    // A folder's lines inside it: its folders, but not a sample folder, which is a top line of its own.
    let inside = |i: usize| f.children[i].iter().copied().filter(|&c| !is_top[c as usize]);
    let mut pick_of: HashMap<u32, bool> = HashMap::new();
    for p in ask.picks {
        if let Some(at) = f.find(&p.path) {
            pick_of.insert(at, p.just_this);
        }
    }
    let hidden: HashSet<u32> = ask.hidden.iter().filter_map(|h| f.find(h)).collect();
    // Children come after their parents: one pass from the end adds each folder into the folder it's in.
    let mut all = vec![0u64; n];
    let mut shown = vec![0u64; n];
    let mut used = vec![0u64; n];
    let mut own_shown = vec![0u64; n];
    let mut own_used = vec![0u64; n];
    let mut marked = vec![false; n];
    let mut pick_below = vec![false; n];
    for i in (0..n).rev() {
        all[i] += u64::from(f.nodes[i].sounds);
        let (s, u) = match counts {
            Some(c) => (
                u64::from(c.sounds.get(i).copied().unwrap_or(0)),
                u64::from(c.used.as_ref().and_then(|u| u.get(i)).copied().unwrap_or(0)),
            ),
            None => (u64::from(f.nodes[i].sounds), 0),
        };
        own_shown[i] = s;
        own_used[i] = u;
        shown[i] += s;
        used[i] += u;
        pick_below[i] |= pick_of.contains_key(&(i as u32));
        marked[i] |= pick_below[i] || hidden.contains(&(i as u32));
        if let Some(p) = f.nodes[i].parent
            && !is_top[i]
        {
            let p = p as usize;
            all[p] += all[i];
            marked[p] |= marked[i];
            pick_below[p] |= pick_below[i];
            // A hidden folder's sounds don't show from the folders above it.
            if !hidden.contains(&(i as u32)) {
                shown[p] += shown[i];
                used[p] += used[i];
            }
        }
    }
    let words: Vec<String> = ask.search.split_whitespace().map(str::to_lowercase).collect();
    let mut out = Vec::new();
    let mut tops: Vec<(String, usize)> =
        (0..n).filter(|&i| is_top[i] && all[i] > 0).map(|i| (f.nodes[i].path.clone(), i)).collect();
    tops.sort_by(|a, b| by_letters(&a.0, &b.0));
    let walk = Walk {
        f,
        counts,
        ask: &ask,
        words: &words,
        inside: &inside,
        all: &all,
        shown: &shown,
        used: &used,
        own_shown: &own_shown,
        own_used: &own_used,
        marked: &marked,
        pick_below: &pick_below,
        pick_of: &pick_of,
        hidden: &hidden,
    };
    for (_, i) in tops {
        walk.line(i, 0, true, false, false, &mut out);
    }
    out
}

/// What [`rows`] works out once and walks the tree with.
struct Walk<'a, I: Fn(usize) -> It, It: Iterator<Item = u32>> {
    f: &'a Folders,
    counts: Option<&'a FolderCounts>,
    ask: &'a TreeAsk<'a>,
    words: &'a [String],
    inside: &'a I,
    all: &'a [u64],
    shown: &'a [u64],
    used: &'a [u64],
    own_shown: &'a [u64],
    own_used: &'a [u64],
    marked: &'a [bool],
    /// A folder in each folder's branch (it too) is picked.
    pick_below: &'a [bool],
    pick_of: &'a HashMap<u32, bool>,
    hidden: &'a HashSet<u32>,
}

impl<I: Fn(usize) -> It, It: Iterator<Item = u32>> Walk<'_, I, It> {
    /// The line of the chain that starts at folder `i`, `depth` in, then the lines inside it (when it's open), into
    /// `out`. Whether any line was put in (with words in the search box, a line shows only when it or a line inside
    /// it has them). `top` says it's a top line, which names its whole path; `within` that a folder above it is
    /// picked with the folders inside it; `hidden_above` that one above it is hidden.
    fn line(
        &self,
        i: usize,
        depth: usize,
        top: bool,
        within: bool,
        hidden_above: bool,
        out: &mut Vec<FolderRow>,
    ) -> bool {
        let f = self.f;
        // Folders that hold no sound and one folder with sounds are one line with it: "Vengeance › Vol 2 › Kicks".
        let mut chain = vec![i];
        let mut last = i;
        loop {
            if f.nodes[last].sounds > 0 {
                break;
            }
            let mut with: Vec<u32> = (self.inside)(last).filter(|&c| self.all[c as usize] > 0).collect();
            if with.len() != 1 {
                break;
            }
            let c = with.pop().unwrap_or(NONE) as usize;
            // A picked or hidden folder ends a chain, so what's picked is what the line says.
            if self.pick_of.contains_key(&(last as u32)) || self.hidden.contains(&(last as u32)) {
                break;
            }
            chain.push(c);
            last = c;
        }
        // A top line names its whole path.
        let label = if top {
            f.nodes[last].path.clone()
        } else {
            chain.iter().map(|&c| f.nodes[c].name.as_str()).collect::<Vec<_>>().join(CHAIN)
        };
        let hidden = self.hidden.contains(&(last as u32)) || chain.iter().any(|c| self.hidden.contains(&(*c as u32)));
        let pick = self.pick_of.get(&(last as u32)).copied();
        let mut kids: Vec<(String, usize)> = (self.inside)(last)
            .filter(|&c| self.all[c as usize] > 0)
            .map(|c| (f.nodes[c as usize].name.clone(), c as usize))
            .collect();
        kids.sort_by(|a, b| by_letters(&a.0, &b.0));
        let just = pick == Some(true);
        let (count, used) =
            if just { (self.own_shown[last], self.own_used[last]) } else { (self.shown[last], self.used[last]) };
        let empty = self.counts.is_some() && count == 0;
        // Shown while the other filters leave it sounds, or it's picked, hidden or holds a folder that is.
        if empty && !self.marked[last] && !hidden_above {
            return false;
        }
        let picked = match pick {
            Some(true) => Picked::JustThis,
            Some(false) => Picked::Yes,
            None if within => Picked::Within,
            None if kids.iter().any(|&(_, c)| self.pick_below[c]) => Picked::Part,
            None => Picked::No,
        };
        let path = f.nodes[last].path.clone();
        let searching = !self.words.is_empty();
        let lower = label.to_lowercase();
        let matches = !searching || self.words.iter().all(|w| lower.contains(w.as_str()));
        let open = searching || self.ask.open.contains(&key(&path));
        let row = FolderRow {
            path,
            label,
            depth,
            count: if self.counts.is_some() { short_count(count as usize) } else { String::new() },
            used: match self.counts.and_then(|c| c.used.as_ref()) {
                Some(_) => format!("{} used", short_count(used as usize)),
                None => String::new(),
            },
            has_children: kids.iter().any(|&(_, c)| self.would_show(c, hidden || hidden_above)),
            open,
            picked,
            hidden: hidden || hidden_above,
            away: top && self.ask.away.contains(&key(&f.nodes[i].path)),
            empty,
        };
        let at = out.len();
        out.push(row);
        let mut any_inside = false;
        if open {
            let within = within || pick == Some(false);
            let hidden_above = hidden_above || hidden;
            for (_, c) in kids {
                // Searching, a matching line shows the lines inside it only where they match too.
                any_inside |= self.line(c, depth + 1, false, within, hidden_above, out);
            }
        }
        if searching && !matches && !any_inside {
            out.truncate(at);
            return false;
        }
        if searching && !any_inside {
            out[at].open = false;
        }
        true
    }

    /// Whether folder `c`'s line would show inside an opened folder (`hidden_above` when that one is hidden).
    fn would_show(&self, c: usize, hidden_above: bool) -> bool {
        self.counts.is_none() || self.shown[c] > 0 || self.marked[c] || hidden_above
    }
}

/// What the Folders chip says of the picks: the first's name, and how many more ("UKG Snares + 1"); "" for none.
pub fn picked_words(picks: &[FolderPick]) -> String {
    let Some(first) = picks.first() else { return String::new() };
    let name = last_name(&first.path);
    match picks.len() {
        1 => name.to_owned(),
        n => format!("{name} + {}", n - 1),
    }
}

/// The line under the tree while folders are picked: how many sounds the picks show ("1 sound", "1,204 sounds").
pub fn shown_words(n: usize) -> String {
    match n {
        1 => "1 sound in the picked folders".to_owned(),
        n => format!("{} sounds in the picked folders", mo_library::songs::thousands(n)),
    }
}

/// A folder's own name, the last of its path ("Kicks"); a drive's top whole ("I:\\").
pub fn last_name(path: &str) -> &str {
    path.trim_end_matches(['\\', '/'])
        .rsplit(['\\', '/'])
        .find(|s| !s.is_empty())
        .filter(|s| !s.ends_with(':'))
        .unwrap_or(path)
}

/// Picks `path` with the folders inside it, or takes its pick off when it's picked (either way).
pub fn toggle_pick(picks: &mut Vec<FolderPick>, path: &str) {
    let k = key(path);
    if let Some(at) = picks.iter().position(|p| key(&p.path) == k) {
        picks.remove(at);
    } else {
        picks.push(FolderPick { path: path.to_owned(), just_this: false });
    }
}

/// Picks `path` alone (not the folders inside it), or picks it with them again when it's picked alone.
pub fn toggle_just_this(picks: &mut Vec<FolderPick>, path: &str) {
    let k = key(path);
    match picks.iter_mut().find(|p| key(&p.path) == k) {
        Some(p) => p.just_this = !p.just_this,
        None => picks.push(FolderPick { path: path.to_owned(), just_this: true }),
    }
}

/// Hides `path`, or shows it again when it's hidden. A folder hidden is taken off the picks.
pub fn toggle_hidden(hidden: &mut Vec<String>, picks: &mut Vec<FolderPick>, path: &str) {
    let k = key(path);
    if let Some(at) = hidden.iter().position(|h| key(h) == k) {
        hidden.remove(at);
    } else {
        hidden.push(path.to_owned());
        // A pick inside it would show nothing: it goes with the pick of the folder itself.
        let inside = format!("{k}\\");
        picks.retain(|p| {
            let pk = key(&p.path);
            pk != k && !pk.starts_with(&inside)
        });
    }
}

/// The picks as their setting keeps them: a line each, `+` before a folder picked with the folders inside it, `=`
/// before one picked alone.
pub fn encode_picks(picks: &[FolderPick]) -> String {
    picks.iter().map(|p| format!("{}{}", if p.just_this { '=' } else { '+' }, p.path)).collect::<Vec<_>>().join("\n")
}

/// The picks from their setting ([`encode_picks`]); a line that isn't one is passed over.
pub fn decode_picks(text: &str) -> Vec<FolderPick> {
    text.lines()
        .filter_map(|l| {
            let just_this = match l.chars().next()? {
                '+' => false,
                '=' => true,
                _ => return None,
            };
            let path = l[1..].trim();
            (!path.is_empty()).then(|| FolderPick { path: path.to_owned(), just_this })
        })
        .collect()
}

/// Folders as a setting keeps them, a line each: hidden folders and open branches.
pub fn encode_paths<S: AsRef<str>>(paths: impl IntoIterator<Item = S>) -> String {
    paths.into_iter().map(|p| p.as_ref().to_owned()).collect::<Vec<_>>().join("\n")
}

/// Folders from their setting ([`encode_paths`]).
pub fn decode_paths(text: &str) -> Vec<String> {
    text.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned).collect()
}

/// Where the settings keep the tree's picks, hidden folders, open branches and whether it's folded away.
pub const PICKS_KEY: &str = "sounds_folders_picked";
pub const HIDDEN_KEY: &str = "sounds_folders_hidden";
pub const OPEN_KEY: &str = "sounds_folders_open";
pub const FOLDED_KEY: &str = "sounds_folders_folded";

#[cfg(test)]
mod tests {
    use super::*;

    fn folders(paths: &[&str]) -> Folders {
        Folders::new(paths.iter().copied())
    }

    fn ask<'a>(
        picks: &'a [FolderPick],
        hidden: &'a [String],
        open: &'a HashSet<String>,
        tops: &'a [String],
    ) -> TreeAsk<'a> {
        TreeAsk { picks, hidden, open, search: "", tops, away: Box::leak(Box::default()) }
    }

    fn labels(rows: &[FolderRow]) -> Vec<String> {
        rows.iter().map(|r| format!("{}{} {}", "  ".repeat(r.depth), r.label, r.count)).collect()
    }

    fn all_open(f: &Folders) -> HashSet<String> {
        f.nodes.iter().map(|n| key(&n.path)).collect()
    }

    const PATHS: [&str; 7] = [
        r"I:\Samples\Packs\UKG Snares\a.wav",
        r"I:\Samples\Packs\UKG Snares\b.wav",
        r"I:\Samples\Packs\Vengeance\Vol 2\Kicks\k.wav",
        r"I:\Samples\Packs\Vengeance\Vol 2\Kicks\Hard\h.wav",
        r"i:\samples\BOUNCES\x.wav",
        r"G:\Studio\909\c.wav",
        r"K:\Old\d.wav",
    ];

    #[test]
    fn folders_are_told_apart_capitals_aside_and_each_is_made_once() {
        let f = folders(&PATHS);
        assert_eq!(f.of_sound(0), f.of_sound(1));
        assert_eq!(f.find(r"i:/SAMPLES/packs/ukg snares/"), Some(f.of_sound(0)));
        // Each folder named as first met: the top two from the first path, BOUNCES from its own.
        assert_eq!(f.path(f.find(r"I:\Samples\Bounces").unwrap()), r"i:\samples\BOUNCES");
        assert_eq!(f.path(f.find(r"I:").unwrap()), r"I:\");
        // I:, Samples, Packs, UKG Snares, Vengeance, Vol 2, Kicks, Hard, BOUNCES; G:, Studio, 909; K:, Old.
        assert_eq!(f.len(), 14);
        assert_eq!(folders(&["/home/me/a.wav", "bare.wav"]).len(), 3);
    }

    #[test]
    fn the_tree_starts_at_the_sample_folders_and_folds_chains() {
        let f = folders(&PATHS);
        let open = all_open(&f);
        let tops = vec![r"I:\Samples\".to_owned(), r"G:\Studio".to_owned()];
        let rows = rows(&f, None, ask(&[], &[], &open, &tops));
        assert_eq!(
            labels(&rows),
            vec![
                // A sample folder that holds one folder and no sound is one line with it too.
                r"G:\Studio\909 ",
                r"I:\Samples ",
                "  BOUNCES ",
                "  Packs ",
                "    UKG Snares ",
                "    Vengeance › Vol 2 › Kicks ",
                "      Hard ",
                r"K:\Old ",
            ]
        );
        assert_eq!(rows[5].path, r"I:\Samples\Packs\Vengeance\Vol 2\Kicks");
        assert!(rows[5].has_children && !rows[6].has_children);
    }

    #[test]
    fn counts_add_up_the_folders_inside_and_a_folder_with_none_left_goes() {
        let f = folders(&PATHS);
        let open = all_open(&f);
        let tops = vec![r"I:\Samples".to_owned()];
        // Only the snares and the hard kick are let through by the other filters.
        let mut sounds = vec![0; f.len()];
        sounds[f.of_sound(0) as usize] = 2;
        sounds[f.of_sound(3) as usize] = 1;
        let mut used = vec![0; f.len()];
        used[f.of_sound(0) as usize] = 1;
        let counts = FolderCounts { of: f.stamp(), sounds, used: Some(used) };
        let rows = rows(&f, Some(&counts), ask(&[], &[], &open, &tops));
        assert_eq!(
            labels(&rows),
            vec![r"I:\Samples 3", "  Packs 3", "    UKG Snares 2", "    Vengeance › Vol 2 › Kicks 1", "      Hard 1"]
        );
        assert_eq!(rows[0].used, "1 used");
        // A hidden folder still shows (to be shown again), and its sounds leave the count above it.
        let hidden = vec![r"I:\Samples\Packs\UKG Snares".to_owned()];
        let rows2 = super::rows(&f, Some(&counts), ask(&[], &hidden, &open, &tops));
        assert_eq!(labels(&rows2)[..3], [r"I:\Samples 1", "  Packs 1", "    UKG Snares 2"]);
        assert!(rows2[2].hidden);
        // A picked folder with nothing left still shows, so it can be taken off; the folder above it is part picked.
        let picks = vec![FolderPick { path: r"I:\Samples\BOUNCES".into(), just_this: false }];
        let rows3 = super::rows(&f, Some(&counts), ask(&picks, &[], &open, &tops));
        assert_eq!(rows3[1].label, "BOUNCES");
        assert!(rows3[1].empty);
        // Counts of another making of the folders (the sounds read again since) are taken for none.
        let stale = FolderCounts { of: f.stamp() + 1_000_000, ..counts.clone() };
        let rows4 = super::rows(&f, Some(&stale), ask(&[], &[], &open, &tops));
        assert!(rows4.iter().all(|r| r.count.is_empty()));
        assert_eq!(rows3[1].picked, Picked::Yes);
        assert_eq!(rows3[0].picked, Picked::Part);
    }

    #[test]
    fn closed_branches_hide_their_lines_and_the_search_opens_what_it_finds() {
        let f = folders(&PATHS);
        let tops = vec![r"I:\Samples".to_owned()];
        let closed = HashSet::new();
        let rows = rows(&f, None, ask(&[], &[], &closed, &tops));
        assert_eq!(labels(&rows), vec![r"G:\Studio\909 ", r"I:\Samples ", r"K:\Old "]);
        let mut a = ask(&[], &[], &closed, &tops);
        a.search = "hard";
        assert_eq!(
            labels(&super::rows(&f, None, a)),
            vec![r"I:\Samples ", "  Packs ", "    Vengeance › Vol 2 › Kicks ", "      Hard "]
        );
        a.search = "snares";
        let found = super::rows(&f, None, a);
        assert_eq!(found.last().map(|r| (r.label.as_str(), r.open)), Some(("UKG Snares", false)));
    }

    #[test]
    fn the_mask_lets_through_picks_with_their_insides_and_keeps_out_hidden_folders() {
        let f = folders(&PATHS);
        let pick = |p: &str, just_this| FolderPick { path: p.into(), just_this };
        let allowed = |m: &FolderMask| (0..PATHS.len()).filter(|&i| m.allows(f.of_sound(i))).collect::<Vec<_>>();
        assert!(f.mask(&[], &[], &[]).is_none());
        let m = f.mask(&[pick(r"I:\Samples\Packs\Vengeance", false)], &[], &[]).unwrap();
        assert_eq!(allowed(&m), vec![2, 3]);
        let m = f.mask(&[pick(r"I:\Samples\Packs\Vengeance\Vol 2\Kicks", true)], &[], &[]).unwrap();
        assert_eq!(allowed(&m), vec![2]);
        let m = f
            .mask(&[pick(r"I:\Samples", false)], &[r"i:\samples\packs\vengeance\vol 2\kicks\hard".into()], &[])
            .unwrap();
        assert_eq!(allowed(&m), vec![0, 1, 2, 4]);
        let m = f.mask(&[], &[r"I:\Samples\Packs".into()], &[]).unwrap();
        assert_eq!(allowed(&m), vec![4, 5, 6]);
        assert!(m.allows(NONE));
        // A folder no sound is in lets nothing through.
        let m = f.mask(&[pick(r"Z:\Gone", false)], &[], &[]).unwrap();
        assert!(allowed(&m).is_empty());
        // A sample folder inside another is a top of its own: the pick and the hide of the one above don't reach it.
        let tops = [r"I:\Samples".to_owned(), r"I:\Samples\Packs\Vengeance".to_owned()];
        let m = f.mask(&[pick(r"I:\Samples", false)], &[], &tops).unwrap();
        assert_eq!(allowed(&m), vec![0, 1, 4]);
        let m = f.mask(&[], &[r"I:\Samples".into()], &tops).unwrap();
        assert_eq!(allowed(&m), vec![2, 3, 5, 6]);
    }

    #[test]
    fn picks_toggle_and_keep_through_their_setting() {
        let mut picks = Vec::new();
        toggle_pick(&mut picks, r"I:\A");
        toggle_just_this(&mut picks, r"i:\a");
        toggle_just_this(&mut picks, r"I:\B");
        assert_eq!(
            picks,
            vec![
                FolderPick { path: r"I:\A".into(), just_this: true },
                FolderPick { path: r"I:\B".into(), just_this: true }
            ]
        );
        assert_eq!(decode_picks(&encode_picks(&picks)), picks);
        assert_eq!(picked_words(&picks), "A + 1");
        let mut hidden = Vec::new();
        toggle_hidden(&mut hidden, &mut picks, r"I:\B\");
        assert_eq!((hidden.len(), picks.len()), (1, 1));
        toggle_hidden(&mut hidden, &mut picks, r"I:\b");
        assert!(hidden.is_empty());
        toggle_pick(&mut picks, r"I:\a");
        assert!(picks.is_empty());
        // Hiding a folder takes off the picks inside it, not those beside it.
        toggle_pick(&mut picks, r"I:\A\B");
        toggle_pick(&mut picks, r"I:\AB");
        toggle_hidden(&mut hidden, &mut picks, r"I:\A");
        assert_eq!(picks, vec![FolderPick { path: r"I:\AB".into(), just_this: false }]);
        assert_eq!(decode_paths(&encode_paths(["x", "", "y"])), vec!["x", "y"]);
        assert_eq!(last_name(r"I:\"), r"I:\");
        assert_eq!(shown_words(1), "1 sound in the picked folders");
        assert_eq!(shown_words(1204), "1,204 sounds in the picked folders");
        assert_eq!(last_name(r"I:\Samples\Kicks\"), "Kicks");
    }
}
