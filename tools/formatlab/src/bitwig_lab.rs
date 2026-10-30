//! Lays out a Bitwig file for Format Lab: its metadata (the summary in "structure" decoded too), its object tree, and
//! the files in its zip. An object is shown where Bitwig first wrote it; a reference elsewhere shows where that is.

use std::collections::{HashMap, HashSet};
use std::io::Read;

use mo_formats::bitwig::{Document, ObjId, Slot, Value, decode, is_bitwig};

use crate::lab::{self, Data, Format, Item, LabFile};
use crate::registry;

/// The field that holds an object's own id.
const F_ID: u32 = 0xf;
/// How many of a list's numbers or texts are shown.
const SHOWN: usize = 32;
/// How deep a Bitwig document inside a metadata blob is decoded too ("structure" is one level). A damaged or made-up
/// file could nest them thousands deep; deeper ones are shown as bytes.
const NESTED: usize = 4;

pub fn read(raw: &[u8], name: &str) -> Result<LabFile, String> {
    let doc = decode(raw).map_err(|_| format!("{name} isn't a Bitwig file"))?;
    let mut items = Vec::new();
    Walker { doc: &doc, items: &mut items, paths: HashMap::new(), nested: 0 }.all("", "", 0);
    zip(&doc, &mut items);
    Ok(LabFile {
        name: name.to_owned(),
        format: Format::Bitwig,
        version: doc.meta_str("application_version_name").to_owned(),
        items,
        problem: doc.body_error.clone(),
        warning: None,
    })
}

/// A field's label: its id, and its name when the registry has one.
fn field_label(id: u32) -> String {
    match registry::bitwig().field(id) {
        Some(f) => format!("0x{id:x} {}", f.name),
        None => format!("0x{id:x}"),
    }
}

fn field_name(id: u32) -> String {
    registry::bitwig().field(id).map_or_else(|| format!("0x{id:x}"), |f| f.name.clone())
}

pub fn class_label(id: u32) -> String {
    match registry::bitwig().class(id) {
        Some(c) => format!("0x{id:x} {}", c.name),
        None => format!("0x{id:x}"),
    }
}

fn join(path: &str, part: &str) -> String {
    if path.is_empty() { part.to_owned() } else { format!("{path} › {part}") }
}

struct Walker<'a> {
    doc: &'a Document,
    items: &'a mut Vec<Item>,
    /// Where each object was shown, for the references to it.
    paths: HashMap<ObjId, String>,
    /// How many documents this one is inside.
    nested: usize,
}

/// A key and a label's ending for the `n`th copy of something written more than once ("#2"), so each copy can be
/// told apart when two saves are compared; none for the first.
fn again(n: usize) -> (String, String) {
    if n > 1 { (format!("#{n}"), format!(" #{n}")) } else { (String::new(), String::new()) }
}

impl Walker<'_> {
    fn all(&mut self, key_prefix: &str, path_prefix: &str, depth: usize) {
        let mut seen: HashMap<&str, usize> = HashMap::new();
        for (key, value) in &self.doc.meta {
            let n = seen.entry(key.as_str()).or_default();
            *n += 1;
            self.meta(key.as_str(), again(*n), value, key_prefix, path_prefix, depth);
        }
        if let Some(root) = self.doc.root {
            let key = format!("{key_prefix}body");
            let path = join(path_prefix, "body");
            self.items.push(Item::group(
                key.clone(),
                path,
                depth,
                "body".into(),
                class_label(self.doc.obj(root).class),
            ));
            self.paths.insert(root, join(path_prefix, "body"));
            self.fields(root, &key, path_prefix, depth + 1);
        }
    }

    fn meta(
        &mut self,
        key: &str,
        (key_again, again): (String, String),
        value: &Value,
        key_prefix: &str,
        path_prefix: &str,
        depth: usize,
    ) {
        let entry = registry::bitwig().meta_key(key);
        let k = format!("{key_prefix}meta/{key}{key_again}");
        let (label, path) = match entry {
            Some(e) => (format!("meta {key} ({}){again}", e.name), join(path_prefix, &format!("{}{again}", e.name))),
            None => (format!("meta {key}{again}"), join(path_prefix, &format!("meta {key}{again}"))),
        };
        if let Value::Blob(blob) = value {
            let bytes = blob.bytes();
            if self.nested < NESTED
                && is_bitwig(bytes)
                && let Ok(inner) = decode(bytes)
            {
                // A Bitwig document of its own, like the summary of the project's tracks.
                let shown = format!("<{} bytes, a Bitwig document>", lab::count(bytes.len()));
                self.items.push(Item::group(k.clone(), path.clone(), depth, label, shown));
                Walker { doc: &inner, items: self.items, paths: HashMap::new(), nested: self.nested + 1 }.all(
                    &format!("{k}/"),
                    &path,
                    depth + 1,
                );
                return;
            }
        }
        self.value(value, k, path, depth, label);
    }

    fn fields(&mut self, o: ObjId, key: &str, path: &str, depth: usize) {
        let mut seen: HashMap<u32, usize> = HashMap::new();
        for field in &self.doc.obj(o).fields {
            let n = seen.entry(field.id).or_default();
            *n += 1;
            let (key_again, again) = again(*n);
            self.value(
                &field.value,
                format!("{key}/0x{:x}{key_again}", field.id),
                join(path, &(field_name(field.id) + &again)),
                depth,
                field_label(field.id) + &again,
            );
        }
    }

    fn slot(&mut self, slot: Slot, key: String, path: String, depth: usize, label: String) {
        match slot {
            Slot::Null => self.items.push(Item::new(key, path, depth, label, "null".into(), Data::None)),
            Slot::Obj(child) => {
                self.paths.insert(child, path.clone());
                let shown = class_label(self.doc.obj(child).class);
                self.items.push(Item::group(key.clone(), path.clone(), depth, label, shown));
                self.fields(child, &key, &path, depth + 1);
            }
            Slot::Ref(n) => {
                let target = self.doc.resolve(slot);
                let shown = match target {
                    Some(t) => {
                        let at = self.paths.get(&t).cloned().unwrap_or_else(|| format!("object #{n}"));
                        format!("→ {at} ({})", class_label(self.doc.obj(t).class))
                    }
                    None => format!("→ object #{n}"),
                };
                self.items.push(Item::new(key, path, depth, label, shown, Data::None));
            }
        }
    }

    fn value(&mut self, v: &Value, key: String, path: String, depth: usize, label: String) {
        let (shown, data) = match v {
            Value::Obj(s) | Value::BareRef(s) => return self.slot(*s, key, path, depth, label),
            Value::Named(s, text) => {
                let (k, p) = (format!("{key}/text"), format!("{path} text"));
                self.slot(*s, key, path, depth, label);
                let t = text.as_str().to_owned();
                self.items.push(Item::new(k, p, depth + 1, "text".into(), lab::quote(&t), Data::Text(t)));
                return;
            }
            Value::List(list) => {
                self.items.push(Item::group(
                    key.clone(),
                    path.clone(),
                    depth,
                    label,
                    format!("list of {}", list.len()),
                ));
                let mut keys = HashSet::new();
                for (i, &s) in list.iter().enumerate() {
                    // An object with its own id keeps its key when others are added or removed before it.
                    let own = match s {
                        Slot::Obj(o) => match self.doc.obj(o).get(F_ID) {
                            Some(Value::Uuid(g)) => Some(format!("[id {}]", lab::hex(&g[..4]).replace(' ', ""))),
                            _ => None,
                        },
                        _ => None,
                    };
                    let element = match own {
                        Some(k) if keys.insert(k.clone()) => k,
                        _ => format!("[{i}]"),
                    };
                    self.slot(s, format!("{key}/{element}"), format!("{path}[{i}]"), depth + 1, format!("[{i}]"));
                }
                return;
            }
            Value::Map(map) => {
                self.items.push(Item::group(key.clone(), path.clone(), depth, label, format!("map of {}", map.len())));
                let mut seen: HashMap<&str, usize> = HashMap::new();
                for (k, s) in map {
                    let k = k.as_str();
                    let n = seen.entry(k).or_default();
                    *n += 1;
                    let (key_again, again) = again(*n);
                    self.slot(
                        *s,
                        format!("{key}/{{{k}}}{key_again}"),
                        format!("{path}{{{k}}}{again}"),
                        depth + 1,
                        format!("{{{k}}}{again}"),
                    );
                }
                return;
            }
            Value::Blob(blob) => {
                let b = blob.bytes().to_vec();
                (lab::bytes(&b), Data::Bytes(b))
            }
            Value::Null => ("null".into(), Data::None),
            Value::Str(t) => (lab::quote(t.as_str()), Data::Text(t.as_str().to_owned())),
            Value::U8(n) | Value::Bool(n) => (n.to_string(), Data::Int(i64::from(*n))),
            Value::I16(n) => (n.to_string(), Data::Int(i64::from(*n))),
            Value::I32(n) => (n.to_string(), Data::Int(i64::from(*n))),
            Value::I64(n) => (n.to_string(), Data::Int(*n)),
            Value::F32(f) => (lab::float(f64::from(*f)), Data::Float(f64::from(*f))),
            Value::F64(f) => (lab::float(*f), Data::Float(*f)),
            Value::Uuid(g) => {
                let h = lab::hex(g).replace(' ', "");
                let (a, rest) = h.split_at(8);
                let (b, rest) = rest.split_at(4);
                let (c, rest) = rest.split_at(4);
                let (d, e) = rest.split_at(4);
                let text = format!("{a}-{b}-{c}-{d}-{e}");
                (text.clone(), Data::Text(text))
            }
            Value::Ints(a) => {
                (list(a.iter().map(i32::to_string), a.len()), Data::Ints(a.iter().map(|&x| x.into()).collect()))
            }
            Value::Floats(a) => floats(a.iter().map(|&x| f64::from(x))),
            Value::Color(a) => floats(a.iter().map(|&x| f64::from(x))),
            Value::Doubles(a) => floats(a.iter().copied()),
            Value::Strings(a) => (
                list(a.iter().map(|t| lab::quote(t.as_str())), a.len()),
                Data::Texts(a.iter().map(|t| t.as_str().to_owned()).collect()),
            ),
        };
        self.items.push(Item::new(key, path, depth, label, shown, data));
    }
}

/// The first of a list's values, in brackets.
fn list(shown: impl Iterator<Item = String>, len: usize) -> String {
    let more = if len > SHOWN { ", …" } else { "" };
    format!("[{}{more}]", shown.take(SHOWN).collect::<Vec<_>>().join(", "))
}

fn floats(values: impl ExactSizeIterator<Item = f64> + Clone) -> (String, Data) {
    let len = values.len();
    (list(values.clone().map(lab::float), len), Data::Floats(values.collect()))
}

/// The files in the project's zip (plugin states), each with its bytes.
fn zip(doc: &Document, items: &mut Vec<Item>) {
    let Some(raw) = doc.zip() else { return };
    let Ok(mut archive) = zip::ZipArchive::new(std::io::Cursor::new(raw)) else { return };
    items.push(Item::group("zip".into(), "zip".into(), 0, "zip".into(), format!("{} files", archive.len())));
    for i in 0..archive.len() {
        let Ok(mut file) = archive.by_index(i) else { continue };
        let name =
            file.name().map_or_else(|_| String::from_utf8_lossy(file.name_raw()).into_owned(), |n| n.into_owned());
        let mut bytes = Vec::new();
        if file.read_to_end(&mut bytes).is_err() {
            bytes.clear();
        }
        items.push(Item::new(
            format!("zip/{name}"),
            format!("zip › {name}"),
            1,
            name,
            lab::bytes(&bytes),
            Data::Bytes(bytes),
        ));
    }
}
