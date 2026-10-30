//! Lays out an FL Studio project for Format Lab: every event in file order, under the part of the project it belongs to
//! (the project, a channel, a pattern, the arrangement, a mixer insert or one of its slots), with the registry's layouts
//! and a Fruity Wrapper's records taken apart.

use std::collections::{HashMap, HashSet};

use mo_formats::fl::events::{
    self, EV_CHANNEL_NEW, EV_INSERT_FLAGS, EV_INSERT_NAME, EV_MIXER_SLOT, EV_PATTERN_NEW, EV_PLAYLIST,
    EV_PLUGIN_INTERNAL, EV_PLUGIN_STATE, Event,
};

use crate::lab::{self, Data, Format, Item, LabFile};
use crate::registry::{self, Entry, LayoutField, Number};

/// The Fruity Wrapper's record that says what kind of plugin it holds, the one with the plugin's own state, and the
/// kind that's a VST3.
const WRAPPER_FORMAT: u32 = 50;
const WRAPPER_STATE: u32 = 53;
const FORMAT_VST3: u8 = 8;

pub fn read(raw: &[u8], name: &str) -> Result<LabFile, String> {
    let flp = events::load(raw).map_err(|e| e.to_string())?;
    // Only what's in the file: the reader adds events of its own (an old FL's step sequencer dots read as notes).
    let in_file: Vec<&Event> = flp.events.iter().filter(|e| e.offset.is_some()).collect();
    let scopes = scopes(&in_file);
    let reg = registry::fl_studio();
    let mut items = Vec::new();
    let mut started = HashSet::new();
    let mut counts: HashMap<(&str, u8), usize> = HashMap::new();
    let mut plugin_types: HashMap<&str, String> = HashMap::new();
    let mut last = None;
    for (e, scope) in in_file.iter().zip(&scopes) {
        let scope = scope.as_str();
        if last != Some(scope) {
            if started.insert(scope) {
                items.push(Item::group(scope.into(), scope.into(), 0, scope.into(), String::new()));
            }
            last = Some(scope);
        }
        let n = counts.entry((scope, e.id)).or_default();
        *n += 1;
        let entry = reg.event(u32::from(e.id));
        let again = if *n > 1 { format!(" #{n}") } else { String::new() };
        let label = match entry {
            Some(entry) => format!("{} {}{again}", e.id, entry.name),
            None => format!("{}{again}", e.id),
        };
        let key = format!("{scope}/{}{}", e.id, again.replace(' ', ""));
        let path = format!("{scope} › {label}");
        let data: &[u8] = &e.data;

        // Ids below 192 are numbers.
        if e.id < 192 {
            let v = e.int();
            items.push(Item::new(key, path, 1, label, v.to_string(), Data::Int(i64::from(v))));
            continue;
        }
        let text = match entry.and_then(|e| e.kind.as_deref()) {
            Some("text") => Some(flp.text(data)),
            Some("ascii") => {
                Some(data.iter().map(|&b| char::from(b)).collect::<String>().trim_end_matches('\0').to_owned())
            }
            _ if entry.is_none() && looks_like_text(data) => Some(flp.text(data)),
            _ => None,
        };
        if let Some(text) = text {
            if e.id == EV_PLUGIN_INTERNAL {
                plugin_types.insert(scope, text.clone());
            }
            items.push(Item::new(key, path, 1, label, lab::quote(&text), Data::Text(text)));
            continue;
        }

        let mut parts = Vec::new();
        if let Some(entry) = entry {
            layout(entry, data, flp.major, &key, &path, &mut parts)?;
        }
        if e.id == EV_PLUGIN_STATE && plugin_types.get(scope).is_some_and(|t| t == "Fruity Wrapper") {
            wrapper(data, &key, &path, &mut parts);
        }
        let mut item = Item::new(key, path, 1, label, lab::bytes(data), Data::Bytes(data.to_vec()));
        item.is_group = !parts.is_empty();
        items.push(item);
        items.extend(parts);
    }
    Ok(LabFile {
        name: name.to_owned(),
        format: Format::FlStudio,
        version: flp.version.clone(),
        items,
        problem: None,
        warning: flp.warning.map(str::to_owned),
    })
}

/// Which part of the project each event belongs to. Channels and patterns start with their own events (64, 65) and
/// the arrangement with the playlist (233). A mixer insert starts with its flags (236), its name just before them; a
/// slot's events end with the slot's number (98), so they're only known to be the slot's once that comes.
fn scopes(events: &[&Event]) -> Vec<String> {
    let mut scopes = vec![String::new(); events.len()];
    let mut current = "project".to_owned();
    let mut insert: Option<u32> = None;
    let mut held: Vec<usize> = Vec::new();
    let release = |held: &mut Vec<usize>, scopes: &mut [String], to: &str| {
        for i in held.drain(..) {
            if let Some(s) = scopes.get_mut(i) {
                *s = to.to_owned();
            }
        }
    };
    for (i, e) in events.iter().enumerate() {
        match e.id {
            EV_CHANNEL_NEW | EV_PATTERN_NEW | EV_PLAYLIST => {
                release(&mut held, &mut scopes, &current);
                current = match e.id {
                    EV_CHANNEL_NEW => format!("channel {}", e.int()),
                    EV_PATTERN_NEW => format!("pattern {}", e.int()),
                    _ => "arrangement".to_owned(),
                };
                insert = None;
            }
            EV_INSERT_FLAGS => {
                release(&mut held, &mut scopes, &current);
                let n = insert.map_or(0, |n| n + 1);
                insert = Some(n);
                current = format!("insert {n}");
                if i > 0
                    && events.get(i - 1).is_some_and(|p| p.id == EV_INSERT_NAME)
                    && let Some(s) = scopes.get_mut(i - 1)
                {
                    s.clone_from(&current);
                }
                if let Some(s) = scopes.get_mut(i) {
                    s.clone_from(&current);
                }
                continue;
            }
            _ => {}
        }
        let Some(n) = insert else {
            if let Some(s) = scopes.get_mut(i) {
                s.clone_from(&current);
            }
            continue;
        };
        held.push(i);
        if e.id == EV_MIXER_SLOT {
            release(&mut held, &mut scopes, &format!("insert {n}, slot {}", e.int()));
        }
    }
    release(&mut held, &mut scopes, &current);
    scopes
}

/// Text FL didn't say is text: UTF-16 with a terminator and nothing unprintable.
fn looks_like_text(b: &[u8]) -> bool {
    b.len() >= 4 && b.len().is_multiple_of(2) && b.ends_with(&[0, 0]) && lab::as_text(b).is_some_and(|t| !t.is_empty())
}

fn number_text(n: Number) -> (String, Data) {
    match n {
        Number::Int(i) => (i.to_string(), Data::Int(i)),
        Number::Float(f) => (lab::float(f), Data::Float(f)),
    }
}

/// The numbers the registry's layout names, and the data's records with theirs.
fn layout(entry: &Entry, data: &[u8], major: u32, key: &str, path: &str, parts: &mut Vec<Item>) -> Result<(), String> {
    for f in LayoutField::parse(entry.layout.as_deref())? {
        if let Some(v) = f.read(data) {
            let (shown, d) = number_text(v);
            let mut item =
                Item::new(format!("{key}/{}", f.name), format!("{path} › {}", f.name), 2, f.name.clone(), shown, d);
            item.span = Some((f.offset, f.size()));
            parts.push(item);
        }
    }
    let (Some(size), Some(record_layout)) = (entry.record_size(major), entry.record_layout.as_deref()) else {
        return Ok(());
    };
    if size == 0 {
        return Ok(());
    }
    let fields = LayoutField::parse(Some(record_layout))?;
    let at = entry.record_at.unwrap_or(0);
    let records = data.get(at..).unwrap_or_default().chunks_exact(size);
    for (r, record) in records.enumerate() {
        let mut item = Item::new(
            format!("{key}/[{r}]"),
            format!("{path}[{r}]"),
            2,
            format!("[{r}]"),
            lab::bytes(record),
            Data::Bytes(record.to_vec()),
        );
        item.is_group = true;
        item.span = Some((at + r * size, size));
        parts.push(item);
        for f in &fields {
            if let Some(v) = f.read(record) {
                let (shown, d) = number_text(v);
                let mut item = Item::new(
                    format!("{key}/[{r}]/{}", f.name),
                    format!("{path}[{r}] › {}", f.name),
                    3,
                    f.name.clone(),
                    shown,
                    d,
                );
                item.span = Some((f.offset, f.size()));
                parts.push(item);
            }
        }
    }
    Ok(())
}

/// One record of a list FL writes as a u32 version, then (u32 id, u64 size, data) for each.
struct Record {
    id: u32,
    at: usize,
    len: usize,
}

/// The records of the Fruity Wrapper's state, or of the VST3 state inside its record 53, in file order. None when
/// the data isn't laid out that way: a version from 1 to 64, ids up to 0xFFFF, every record fitting, and less than a
/// record's head left over.
fn records(data: &[u8]) -> Option<Vec<Record>> {
    if data.len() < 16 {
        return None;
    }
    let version = u32::from_le_bytes(*data.first_chunk()?);
    if version == 0 || version > 64 {
        return None;
    }
    let mut records = Vec::new();
    let mut pos = 4;
    while pos + 12 <= data.len() {
        let id = u32::from_le_bytes(*data.get(pos..)?.first_chunk()?);
        let size = u64::from_le_bytes(*data.get(pos + 4..)?.first_chunk()?);
        pos += 12;
        if id > 0xFFFF || size > (data.len() - pos) as u64 {
            return None;
        }
        records.push(Record { id, at: pos, len: size as usize });
        pos += size as usize;
    }
    (!records.is_empty() && data.len() - pos < 12).then_some(records)
}

/// A Fruity Wrapper's records, and a VST3 plugin's own records inside its state.
fn wrapper(data: &[u8], key: &str, path: &str, parts: &mut Vec<Item>) {
    let Some(list) = records(data) else { return };
    let reg = registry::fl_studio();
    let vst3 = list.iter().any(|r| r.id == WRAPPER_FORMAT && r.len > 0 && data.get(r.at) == Some(&FORMAT_VST3));
    for r in &list {
        let entry = reg.wrapper_record(r.id);
        let label = match entry {
            Some(e) => format!("wrapper {} {}", r.id, e.name),
            None => format!("wrapper {}", r.id),
        };
        let k = format!("{key}/wrapper {}", r.id);
        let p = format!("{path} › {label}");
        let bytes = data.get(r.at..r.at + r.len).unwrap_or_default();
        parts.push(part(entry, k.clone(), p.clone(), 2, label, bytes, (r.at, r.len)));
        if !vst3 || r.id != WRAPPER_STATE {
            continue;
        }
        let Some(inner) = records(bytes) else { continue };
        for x in inner {
            let e = reg.vst3_record(x.id);
            let l = match e {
                Some(e) => format!("vst3 {} {}", x.id, e.name),
                None => format!("vst3 {}", x.id),
            };
            let b = bytes.get(x.at..x.at + x.len).unwrap_or_default();
            parts.push(part(e, format!("{k}/vst3 {}", x.id), format!("{p} › {l}"), 3, l, b, (x.at, x.len)));
        }
    }
}

fn part(
    entry: Option<&Entry>,
    key: String,
    path: String,
    depth: usize,
    label: String,
    b: &[u8],
    span: (usize, usize),
) -> Item {
    let four = b.first_chunk::<4>().copied().filter(|_| b.len() == 4);
    let (shown, data) = match (entry.and_then(|e| e.kind.as_deref()), four) {
        (Some("utf8"), _) => {
            let s = String::from_utf8_lossy(b).trim_end_matches('\0').to_owned();
            (lab::quote(&s), Data::Text(s))
        }
        (Some("u32"), Some(f)) => {
            let v = u32::from_le_bytes(f);
            (v.to_string(), Data::Int(i64::from(v)))
        }
        (Some("i32"), Some(f)) => {
            let v = i32::from_le_bytes(f);
            (v.to_string(), Data::Int(i64::from(v)))
        }
        (Some("fourcc"), Some(f)) => {
            // A VST2 ID: a number that's also four letters, read most significant byte first ("XfsX" is Serum).
            let id = i32::from_le_bytes(f);
            let letters: String = f.iter().rev().map(|&c| char::from(c)).collect();
            let shown = if letters.chars().all(|c| (' '..='~').contains(&c)) {
                format!("{id} ({})", lab::quote(&letters))
            } else {
                id.to_string()
            };
            (shown, Data::Int(i64::from(id)))
        }
        _ => (lab::bytes(b), Data::Bytes(b.to_vec())),
    };
    let mut item = Item::new(key, path, depth, label, shown, data);
    item.span = Some(span);
    item
}
