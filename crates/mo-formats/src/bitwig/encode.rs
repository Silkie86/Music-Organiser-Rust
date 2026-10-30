//! Writing Bitwig's format back out, and copying objects inside a document (for building projects from a template
//! Bitwig saved).

use std::collections::{HashMap, HashSet};

use super::format::{Document, Field, HEADER, ObjId, Object, Slot, Text, Value};

/// Writes a decoded document back out: the header, the metadata, the object tree from [`Document::root`], then the
/// zip (the file's own unless `zip` is given).
///
/// As Bitwig does, an object is written in full where it's first reached and referenced after that, wherever the
/// decode found it; references are numbered in that order. So objects can be added, removed or copied, and an
/// untouched document comes back byte for byte. Two rules from Bitwig (rule 8): an object is defined where it's first
/// used, and a field that holds one object never holds a "class 1" reference: one already written is a bare
/// reference (0x0b) there. Bitwig can't read a reference in such a field ("Unknown class ID 1"), only in lists and
/// maps: Solomon's requiem_80 had two automation lanes on one Reaktor, and the second lane's reference to it stopped
/// the whole project opening. (A reference read from the file stays as it was.)
pub fn encode(doc: &Document, zip: Option<&[u8]>) -> Result<Vec<u8>, String> {
    let root = match (doc.root, &doc.body_error) {
        (Some(root), None) => root,
        _ => return Err("Only a cleanly decoded document can be written back".into()),
    };
    let mut w = Writer { doc, numbers: HashMap::new(), written: 0, out: Vec::new() };

    // Documents nested in metadata (Bitwig's "structure") have no metadata of their own.
    if doc.meta_end > HEADER {
        w.u32(4);
        w.text(&Text::new("meta"));
        for (key, value) in &doc.meta {
            w.u32(1);
            w.text(key);
            w.out.push(value.type_byte());
            w.value(value.type_byte(), value)?;
        }
    }
    // The end marker and padding, as they were.
    w.out.extend_from_slice(doc.raw.get(doc.meta_end..doc.body_offset).ok_or("the metadata runs into the body")?);
    let meta = std::mem::take(&mut w.out);

    w.number(root)?;
    w.inline(root)?;
    let body = w.out;

    let had_zip = std::str::from_utf8(&doc.header[20..36]).ok().and_then(|s| u64::from_str_radix(s, 16).ok());
    let zip = match zip {
        Some(z) => z,
        None if had_zip.unwrap_or(0) > 0 => &doc.raw[doc.zip_offset..],
        None => &[],
    };
    let body_offset = HEADER + meta.len();
    let zip_offset = if zip.is_empty() { 0 } else { body_offset + body.len() };

    let mut file = Vec::with_capacity(body_offset + body.len() + zip.len());
    file.extend_from_slice(b"BtWg");
    file.extend_from_slice(&doc.header[..12]);
    file.extend_from_slice(format!("{body_offset:08x}{zip_offset:016x}").as_bytes());
    file.extend_from_slice(&doc.header[36..]);
    file.extend_from_slice(&meta);
    file.extend_from_slice(&body);
    file.extend_from_slice(zip);
    Ok(file)
}

struct Writer<'a> {
    doc: &'a Document,
    /// Each object's number, in the order it's written.
    numbers: HashMap<ObjId, u32>,
    /// How many objects are written so far.
    written: u32,
    out: Vec<u8>,
}

impl Writer<'_> {
    fn u32(&mut self, v: u32) {
        self.out.extend_from_slice(&v.to_be_bytes());
    }

    fn text(&mut self, t: &Text) {
        if let Some(raw) = &t.raw {
            let n = if t.wide { 0x8000_0000 | (raw.len() / 2) as u32 } else { raw.len() as u32 };
            self.u32(n);
            self.out.extend_from_slice(raw);
        } else if t.wide {
            let units: Vec<u16> = t.text.encode_utf16().collect();
            self.u32(0x8000_0000 | units.len() as u32);
            self.out.extend(units.iter().flat_map(|u| u.to_be_bytes()));
        } else {
            self.u32(t.text.len() as u32);
            self.out.extend_from_slice(t.text.as_bytes());
        }
    }

    fn target(&self, s: Slot) -> Result<ObjId, String> {
        self.doc.resolve(s).ok_or_else(|| format!("reference to an object that isn't there: {s:?}"))
    }

    fn is_written(&self, s: Slot) -> bool {
        self.doc.resolve(s).and_then(|t| self.numbers.get(&t)).is_some_and(|&n| n <= self.written)
    }

    /// Numbers every object in the order it will be written: in full where it's first reached through an object
    /// slot, by reference after that.
    fn number(&mut self, o: ObjId) -> Result<(), String> {
        self.numbers.insert(o, self.numbers.len() as u32 + 1);
        let doc = self.doc;
        for f in &doc.obj(o).fields {
            // A bare reference to an object not reached yet (its first home was edited away) is where it's written now.
            let slots = match &f.value {
                Value::BareRef(s) => vec![*s],
                v => v.slots(),
            };
            for s in slots.into_iter().filter(|s| *s != Slot::Null) {
                let t = self.target(s)?;
                if !self.numbers.contains_key(&t) {
                    self.number(t)?;
                }
            }
        }
        Ok(())
    }

    fn inline(&mut self, o: ObjId) -> Result<(), String> {
        self.written += 1;
        if self.numbers.get(&o) != Some(&self.written) {
            return Err("objects written out of order".into());
        }
        let doc = self.doc;
        let object = doc.obj(o);
        self.u32(object.class);
        for f in &object.fields {
            self.u32(f.id);
            let ty = match &f.value {
                // First reached here, so written in full, as Bitwig does.
                Value::BareRef(s) if *s != Slot::Null && !self.is_written(*s) => 0x09,
                // Already written: a bare reference, never a class 1 one in a single-object field.
                Value::Obj(s @ Slot::Obj(_)) if self.is_written(*s) => 0x0b,
                v => v.type_byte(),
            };
            self.out.push(ty);
            self.value(ty, &f.value)?;
        }
        self.u32(0);
        Ok(())
    }

    fn slot(&mut self, s: Slot) -> Result<(), String> {
        if s == Slot::Null {
            self.u32(0);
            return Ok(());
        }
        let t = self.target(s)?;
        match self.numbers.get(&t) {
            Some(&n) if n <= self.written => {
                self.u32(1);
                self.u32(n);
                Ok(())
            }
            _ => self.inline(t),
        }
    }

    fn value(&mut self, ty: u8, v: &Value) -> Result<(), String> {
        match (ty, v) {
            (_, Value::U8(b) | Value::Bool(b)) => self.out.push(*b),
            (_, Value::I16(x)) => self.out.extend_from_slice(&x.to_be_bytes()),
            (_, Value::I32(x)) => self.out.extend_from_slice(&x.to_be_bytes()),
            (_, Value::I64(x)) => self.out.extend_from_slice(&x.to_be_bytes()),
            (_, Value::F32(x)) => self.out.extend_from_slice(&x.to_be_bytes()),
            (_, Value::F64(x)) => self.out.extend_from_slice(&x.to_be_bytes()),
            (_, Value::Str(t)) => self.text(t),
            (0x0b, Value::Obj(s) | Value::BareRef(s)) => {
                let n = self.numbers.get(&self.target(*s)?).copied().filter(|&n| n <= self.written);
                self.u32(n.ok_or("reference to an object that isn't written before it")?);
            }
            (_, Value::Obj(s) | Value::BareRef(s)) => self.slot(*s)?,
            (_, Value::Null) => {}
            (_, Value::Blob(b)) => {
                let bytes = self.doc.blob(b);
                self.u32(bytes.len() as u32);
                self.out.extend_from_slice(bytes);
            }
            (_, Value::Ints(a)) => {
                self.u32(a.len() as u32);
                self.out.extend(a.iter().flat_map(|x| x.to_be_bytes()));
            }
            (_, Value::List(l)) => {
                for s in l {
                    self.slot(*s)?;
                }
                self.u32(3);
            }
            (_, Value::Map(m)) => {
                for (key, s) in m {
                    self.out.push(1);
                    self.text(key);
                    self.slot(*s)?;
                }
                self.out.push(0);
            }
            (_, Value::Uuid(g)) => self.out.extend_from_slice(g),
            (_, Value::Color(c)) => self.out.extend(c.iter().flat_map(|x| x.to_be_bytes())),
            (_, Value::Floats(f)) => {
                self.u32(f.len() as u32);
                self.out.extend(f.iter().flat_map(|x| x.to_be_bytes()));
            }
            (_, Value::Doubles(f)) => {
                self.u32(f.len() as u32);
                self.out.extend(f.iter().flat_map(|x| x.to_be_bytes()));
            }
            (_, Value::Strings(l)) => {
                self.u32(l.len() as u32);
                for t in l {
                    self.text(t);
                }
            }
            (_, Value::Named(s, t)) => {
                self.slot(*s)?;
                self.text(t);
            }
        }
        Ok(())
    }
}

/// Every object a value points at: what it holds, lists, maps or refers to, a named object, or a bare reference.
fn targets(v: &Value) -> Vec<Slot> {
    match v {
        Value::BareRef(s) => vec![*s],
        v => v.slots(),
    }
}

/// Field 0x99 is a device's type: copies of a device keep it, while every other id they carry gets a fresh one.
const DEVICE_TYPE: u32 = 0x99;

/// Copies objects inside a document, for building a project out of a template's tracks and devices.
///
/// Everything the document's root reaches when the copier is made is shared: copies link to it rather than copy it
/// (the master track a track routes to, which Bitwig defines inside the first track). So take the objects to copy out
/// of the tree first, or name the one still in it as `besides`, then copy them as often as needed.
pub struct Copier {
    shared: HashSet<ObjId>,
}

impl Copier {
    /// `besides` is an object still in the tree that will be copied: what's only reachable through it isn't shared.
    pub fn new(doc: &Document, besides: Option<ObjId>) -> Copier {
        let mut shared = HashSet::new();
        if let Some(root) = doc.root {
            reach(doc, root, &mut shared, |o| Some(o) == besides);
        }
        Copier { shared }
    }

    /// Whether copies link to this object rather than copy it.
    pub fn shares(&self, o: ObjId) -> bool {
        self.shared.contains(&o)
    }

    /// A copy of `original` and everything only it reaches. References inside the copy point at the copied objects;
    /// ids (instances, tracks, chains, not device types) get fresh values, and other fields holding the same ids follow.
    pub fn copy(&self, doc: &mut Document, original: ObjId) -> ObjId {
        let mut owned = HashSet::new();
        reach(doc, original, &mut owned, |o| self.shared.contains(&o));
        let mut ids: HashMap<[u8; 16], [u8; 16]> = HashMap::new();
        let mut sorted: Vec<ObjId> = owned.iter().copied().collect();
        sorted.sort();
        for &o in &sorted {
            for f in &doc.obj(o).fields {
                if let Value::Uuid(g) = f.value
                    && f.id != DEVICE_TYPE
                {
                    ids.entry(g).or_insert_with(fresh_id);
                }
            }
        }
        let mut c = Copying { owned: &owned, ids: &ids, copies: HashMap::new() };
        c.copy(doc, original)
    }
}

/// Copies `original` and everything only it reaches inside `doc`, sharing the rest. Returns the copy.
pub fn clone_object(doc: &mut Document, original: ObjId) -> ObjId {
    Copier::new(doc, Some(original)).copy(doc, original)
}

/// Every object reachable from `start`, not going into those where `stop` says so.
fn reach(doc: &Document, start: ObjId, seen: &mut HashSet<ObjId>, stop: impl Fn(ObjId) -> bool) {
    let mut todo = vec![start];
    while let Some(o) = todo.pop() {
        if stop(o) || !seen.insert(o) {
            continue;
        }
        for f in &doc.obj(o).fields {
            todo.extend(targets(&f.value).into_iter().filter_map(|s| doc.resolve(s)));
        }
    }
}

struct Copying<'a> {
    owned: &'a HashSet<ObjId>,
    ids: &'a HashMap<[u8; 16], [u8; 16]>,
    copies: HashMap<ObjId, ObjId>,
}

impl Copying<'_> {
    fn copy(&mut self, doc: &mut Document, o: ObjId) -> ObjId {
        if let Some(&done) = self.copies.get(&o) {
            return done;
        }
        let copy = doc.add(Object::new(doc.obj(o).class));
        self.copies.insert(o, copy);
        let fields = doc.obj(o).fields.clone();
        let fields = fields.into_iter().map(|f| Field { id: f.id, value: self.value(doc, f.value) }).collect();
        doc.obj_mut(copy).fields = fields;
        copy
    }

    fn slot(&mut self, doc: &mut Document, s: Slot) -> Slot {
        match doc.resolve(s) {
            None => s,
            Some(t) if self.owned.contains(&t) => Slot::Obj(self.copy(doc, t)),
            Some(t) => Slot::Obj(t),
        }
    }

    fn value(&mut self, doc: &mut Document, v: Value) -> Value {
        match v {
            Value::Obj(s) => Value::Obj(self.slot(doc, s)),
            Value::BareRef(s) => Value::BareRef(self.slot(doc, s)),
            Value::List(l) => Value::List(l.into_iter().map(|s| self.slot(doc, s)).collect()),
            Value::Map(m) => Value::Map(m.into_iter().map(|(k, s)| (k, self.slot(doc, s))).collect()),
            Value::Named(s, t) => Value::Named(self.slot(doc, s), t),
            Value::Uuid(g) => Value::Uuid(self.ids.get(&g).copied().unwrap_or(g)),
            v => v,
        }
    }
}

/// A new random id (a version 4 UUID), for a copied track or device.
pub fn fresh_id() -> [u8; 16] {
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let n = COUNT.fetch_add(1, Ordering::Relaxed);
    let time = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let mut bytes = [0u8; 16];
    for (i, half) in bytes.chunks_mut(8).enumerate() {
        // Each RandomState is seeded afresh by the system, so these differ run to run as well as call to call.
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(n);
        h.write_u128(time);
        h.write_usize(i);
        half.copy_from_slice(&h.finish().to_be_bytes());
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    bytes
}
