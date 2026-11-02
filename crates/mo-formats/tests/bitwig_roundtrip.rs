//! Round trips on random Bitwig files (spec 11.2): thousands of made-up files and object trees, not just the few
//! fixtures in `bitwig_format.rs`.
//!
//! - A random file, written byte by byte the way Bitwig lays it out, decodes cleanly up to the zip and writes back
//!   byte for byte.
//! - A random tree of objects built in code (shared objects, loops, objects nothing reaches) writes out, reads back as
//!   the same tree, and the file it makes writes back byte for byte.
//! - A random file with a field or list entry taken out still writes and reads back as the edited tree, with every
//!   reference pointing where it did (rule 8: objects defined where first used, no "class 1" reference in a field that
//!   holds one object).
//! - A random file that's cut short or has bytes changed never stops the reader.
//!
//! Each test is driven by a list of random numbers that a small builder turns into choices, so when one fails,
//! proptest shrinks the list down to the smallest file that still fails and prints it.

// Test files may unwrap: a test that stops there has failed, which is what it should do.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::HashMap;

use mo_formats::bitwig::{self, Blob, Document, Field, ObjId, Object, Slot, Text, Value};
use proptest::prelude::*;

/// Turns a list of random numbers into choices. When the list runs out every choice is 0, which is always the
/// simplest one (no more fields, no more entries, no object), so whatever is being built comes to an end.
struct Choices<'a> {
    numbers: &'a [u32],
    at: usize,
}

impl Choices<'_> {
    fn new(numbers: &[u32]) -> Choices<'_> {
        Choices { numbers, at: 0 }
    }

    fn raw(&mut self) -> u32 {
        let v = self.numbers.get(self.at).copied().unwrap_or(0);
        self.at += 1;
        v
    }

    /// A number from 0 to `n - 1`.
    fn pick(&mut self, n: u32) -> u32 {
        self.raw() % n
    }

    fn raw64(&mut self) -> u64 {
        (u64::from(self.raw()) << 32) | u64::from(self.raw())
    }
}

/// Object classes to use. Never 0 (none), 1 (a reference) or 3 (the end of a list): those numbers mean something
/// else where an object is expected. The last is "any other".
const CLASSES: [u32; 7] = [0x2e, 0x1dd, 0x4c1, 0x29a6, 2, 0xffff_ffff, 0];

/// Short texts that repeat, as names do in a project (the reader shares them).
const WORDS: [&str; 5] = ["", "Loop Idea", "Kick 01", "Master", "Keys Display"];

/// Value types that hold no objects.
const PLAIN: [u8; 16] =
    [0x01, 0x05, 0x02, 0x03, 0x04, 0x06, 0x07, 0x08, 0x0a, 0x0d, 0x0f, 0x15, 0x16, 0x17, 0x18, 0x19];

/// Objects nested no deeper than this, and no more of them than this, in a random file.
const MAX_DEPTH: u32 = 24;
const MAX_OBJECTS: u32 = 120;

/// Writes a random file in Bitwig's layout, choice by choice.
struct FileMaker<'a> {
    c: Choices<'a>,
    out: Vec<u8>,
    /// Objects written so far, which a reference may point at.
    objects: u32,
}

impl FileMaker<'_> {
    fn u32(&mut self, v: u32) {
        self.out.extend_from_slice(&v.to_be_bytes());
    }

    fn class(&mut self) -> u32 {
        match CLASSES[self.c.pick(CLASSES.len() as u32) as usize] {
            0 => 4 + self.c.raw() % (u32::MAX - 4),
            class => class,
        }
    }

    /// A text of every kind Bitwig writes, and some it shouldn't: long, UTF-16, and bytes that aren't valid text.
    fn text(&mut self) {
        match self.c.pick(7) {
            0 => {
                let w = WORDS[self.c.pick(WORDS.len() as u32) as usize];
                self.u32(w.len() as u32);
                self.out.extend_from_slice(w.as_bytes());
            }
            1 => {
                // Up to 100 letters: some longer than the 64 the reader shares.
                let n = self.c.pick(101);
                let letters: Vec<u8> = (0..n).map(|_| b'a' + self.c.pick(26) as u8).collect();
                self.u32(n);
                self.out.extend_from_slice(&letters);
            }
            2 => {
                let s = ["Café", "Bass ♪", "Ünïcode Pad"][self.c.pick(3) as usize];
                self.u32(s.len() as u32);
                self.out.extend_from_slice(s.as_bytes());
            }
            3 => {
                let s = ["", "Wide", "Bäss ♪ 2"][self.c.pick(3) as usize];
                let units: Vec<u16> = s.encode_utf16().collect();
                self.u32(0x8000_0000 | units.len() as u32);
                self.out.extend(units.iter().flat_map(|u| u.to_be_bytes()));
            }
            4 => {
                // UTF-16 with a lone surrogate.
                self.u32(0x8000_0002);
                self.out.extend_from_slice(&[0xd8, 0x00, 0x00, b'a']);
            }
            5 => {
                // Bytes that aren't UTF-8.
                self.u32(3);
                self.out.extend_from_slice(&[0xff, b'a', 0xc3]);
            }
            _ => {
                let n = self.c.pick(6);
                let units: Vec<u16> = (0..n).map(|_| self.c.raw() as u16).collect();
                self.u32(0x8000_0000 | n);
                self.out.extend(units.iter().flat_map(|u| u.to_be_bytes()));
            }
        }
    }

    /// A value of a type that holds no objects.
    fn plain(&mut self, ty: u8) {
        match ty {
            0x01 => {
                let b = self.c.raw() as u8;
                self.out.push(b);
            }
            0x05 => {
                let b = self.c.pick(2) as u8;
                self.out.push(b);
            }
            0x02 => {
                let v = self.c.raw() as u16;
                self.out.extend_from_slice(&v.to_be_bytes());
            }
            // Floats as any bits, NaNs included: they're written back as the same bits.
            0x03 | 0x06 => {
                let v = self.c.raw();
                self.u32(v);
            }
            0x04 | 0x07 => {
                let v = self.c.raw64();
                self.out.extend_from_slice(&v.to_be_bytes());
            }
            0x08 => self.text(),
            0x0a => {}
            0x0d => {
                let n = self.c.pick(40);
                self.u32(n);
                for _ in 0..n {
                    let b = self.c.raw() as u8;
                    self.out.push(b);
                }
            }
            0x0f | 0x17 => {
                let n = self.c.pick(6);
                self.u32(n);
                for _ in 0..n {
                    let v = self.c.raw();
                    self.u32(v);
                }
            }
            0x15 => {
                for _ in 0..4 {
                    let v = self.c.raw();
                    self.u32(v);
                }
            }
            0x16 => {
                for _ in 0..4 {
                    let v = self.c.pick(1001) as f32 / 1000.0;
                    self.u32(v.to_bits());
                }
            }
            0x18 => {
                let n = self.c.pick(6);
                self.u32(n);
                for _ in 0..n {
                    let v = self.c.raw64();
                    self.out.extend_from_slice(&v.to_be_bytes());
                }
            }
            0x19 => {
                let n = self.c.pick(4);
                self.u32(n);
                for _ in 0..n {
                    self.text();
                }
            }
            _ => unreachable!("not a plain type: {ty:02x}"),
        }
    }

    fn room_for_more(&self, depth: u32) -> bool {
        depth < MAX_DEPTH && self.objects < MAX_OBJECTS
    }

    /// An object slot in a list, map or name: none, a reference back ("class 1"), or a new object.
    fn slot(&mut self, depth: u32) {
        match self.c.pick(3) {
            1 => {
                let n = 1 + self.c.pick(self.objects);
                self.u32(1);
                self.u32(n);
            }
            2 if self.room_for_more(depth) => self.object(depth + 1),
            _ => self.u32(0),
        }
    }

    /// An object: its class, its fields, then field id 0.
    fn object(&mut self, depth: u32) {
        let class = self.class();
        self.u32(class);
        self.objects += 1;
        for _ in 0..self.c.pick(8) {
            // Field ids are never 0, which ends the object. They may repeat.
            let id = 1 + self.c.pick(0x3000);
            self.u32(id);
            // Half the fields hold objects, so the tree grows until it's as big or as deep as allowed.
            if self.c.pick(2) == 0 {
                let ty = PLAIN[self.c.pick(PLAIN.len() as u32) as usize];
                self.out.push(ty);
                self.plain(ty);
                continue;
            }
            match self.c.pick(5) as usize + PLAIN.len() {
                // A field that holds one object holds it in full, or none, or is a bare reference (0x0b) back to one
                // already written: never a "class 1" reference, as Bitwig writes them (rule 8).
                i if i == PLAIN.len() => {
                    self.out.push(0x09);
                    if self.room_for_more(depth) && self.c.pick(2) == 1 {
                        self.object(depth + 1);
                    } else {
                        self.u32(0);
                    }
                }
                i if i == PLAIN.len() + 1 => {
                    self.out.push(0x0b);
                    let n = 1 + self.c.pick(self.objects);
                    self.u32(n);
                }
                i if i == PLAIN.len() + 2 => {
                    self.out.push(0x12);
                    for _ in 0..self.c.pick(5) {
                        self.slot(depth);
                    }
                    self.u32(3);
                }
                i if i == PLAIN.len() + 3 => {
                    self.out.push(0x14);
                    for _ in 0..self.c.pick(4) {
                        self.out.push(1);
                        self.text();
                        self.slot(depth);
                    }
                    self.out.push(0);
                }
                _ => {
                    self.out.push(0x1a);
                    self.slot(depth);
                    self.text();
                }
            }
        }
        self.u32(0);
    }
}

/// A random file: the header, the metadata (or none), padding, one tree of objects, and a zip (or none).
fn random_file(numbers: &[u32]) -> Vec<u8> {
    let mut m = FileMaker { c: Choices::new(numbers), out: Vec::new(), objects: 0 };
    if m.c.pick(4) != 3 {
        m.u32(4);
        m.u32(4);
        m.out.extend_from_slice(b"meta");
        for _ in 0..m.c.pick(5) {
            m.u32(1);
            m.text();
            let ty = PLAIN[m.c.pick(PLAIN.len() as u32) as usize];
            m.out.push(ty);
            m.plain(ty);
        }
    }
    // The end of the metadata, then padding as it comes.
    m.u32(0);
    for _ in 0..m.c.pick(9) {
        let b = m.c.raw() as u8;
        m.out.push(b);
    }
    let meta = std::mem::take(&mut m.out);
    m.object(0);
    let body = std::mem::take(&mut m.out);
    let mut zip = Vec::new();
    if m.c.pick(2) == 1 {
        zip.extend_from_slice(b"PK\x03\x04");
        for _ in 0..m.c.pick(30) {
            zip.push(m.c.raw() as u8);
        }
    }

    let body_offset = bitwig::format::HEADER + meta.len();
    let zip_offset = if zip.is_empty() { 0 } else { body_offset + body.len() };
    let format = ["000100010001", "000500020001", "0f0e0d0c0b0a"][m.c.pick(3) as usize];
    let mut file = Vec::new();
    file.extend_from_slice(b"BtWg");
    file.extend_from_slice(format.as_bytes());
    file.extend_from_slice(format!("{body_offset:08x}{zip_offset:016x}00").as_bytes());
    file.extend_from_slice(&meta);
    file.extend_from_slice(&body);
    file.extend_from_slice(&zip);
    file
}

/// A random tree of objects built in code, on an empty document's header and metadata. Any object may hold or point
/// at any other, itself included, so there are shared objects, loops and objects nothing reaches.
fn random_tree(numbers: &[u32]) -> Document {
    let mut doc = bitwig::decode(random_file(&[])).expect("an empty file");
    let mut c = Choices::new(numbers);
    let n = 1 + c.pick(40);
    doc.objects = (0..n)
        .map(|_| {
            Object::new(match CLASSES[c.pick(CLASSES.len() as u32) as usize] {
                0 => 4 + c.raw() % (u32::MAX - 4),
                class => class,
            })
        })
        .collect();
    let slot = |c: &mut Choices, null: bool| match c.pick(if null { 3 } else { 2 }) {
        2 => Slot::Null,
        _ => Slot::Obj(ObjId(c.pick(n))),
    };
    let text = |c: &mut Choices| match c.pick(3) {
        0 => Text::new(WORDS[c.pick(WORDS.len() as u32) as usize]),
        1 => Text::new(["Café", "Bass ♪", "Ünïcode Pad"][c.pick(3) as usize]),
        _ => Text::new(&"x".repeat(c.pick(100) as usize)),
    };
    for o in 0..n {
        for _ in 0..c.pick(7) {
            let id = 1 + c.pick(0x3000);
            let value = match c.pick(22) {
                0 => Value::U8(c.raw() as u8),
                1 => Value::Bool(c.pick(2) as u8),
                2 => Value::I16(c.raw() as i16),
                3 => Value::I32(c.raw() as i32),
                4 => Value::I64(c.raw64() as i64),
                5 => Value::F32(f32::from_bits(c.raw())),
                6 => Value::F64(f64::from_bits(c.raw64())),
                7 => Value::Str(text(&mut c)),
                8 => Value::Null,
                9 => Value::Blob(Box::new(Blob::new(&c.raw().to_be_bytes()[..c.pick(5) as usize]))),
                10 => Value::Ints((0..c.pick(5)).map(|_| c.raw() as i32).collect()),
                11 => Value::Uuid(std::array::from_fn(|_| c.raw() as u8)),
                12 => Value::Color(std::array::from_fn(|_| c.pick(1001) as f32 / 1000.0)),
                13 => Value::Floats((0..c.pick(5)).map(|_| f32::from_bits(c.raw())).collect()),
                14 => Value::Doubles((0..c.pick(5)).map(|_| f64::from_bits(c.raw64())).collect()),
                15 => Value::Strings((0..c.pick(4)).map(|_| text(&mut c)).collect()),
                16 | 17 => Value::Obj(slot(&mut c, true)),
                18 => Value::BareRef(slot(&mut c, false)),
                19 => Value::List((0..c.pick(5)).map(|_| slot(&mut c, true)).collect()),
                20 => Value::Map((0..c.pick(4)).map(|_| (text(&mut c), slot(&mut c, true))).collect()),
                _ => Value::Named(slot(&mut c, true), text(&mut c)),
            };
            doc.objects[o as usize].fields.push(Field { id, value });
        }
    }
    doc.root = Some(ObjId(0));
    doc
}

/// The tree the root reaches, written out so two documents can be compared whatever order their objects are kept in:
/// each object numbered by when it's first reached, and each link as the number of the object it reaches. An object
/// held in a field and a bare reference to it are the same link (the writer picks which one by what's written first).
fn tree(doc: &Document) -> Vec<String> {
    fn visit(doc: &Document, o: ObjId, order: &mut Vec<ObjId>, number: &mut HashMap<ObjId, usize>) {
        if number.contains_key(&o) {
            return;
        }
        number.insert(o, order.len());
        order.push(o);
        for f in &doc.obj(o).fields {
            for t in f.value.targets().filter_map(|s| doc.resolve(s)) {
                visit(doc, t, order, number);
            }
        }
    }
    let (mut order, mut number) = (Vec::new(), HashMap::new());
    visit(doc, doc.root.expect("a root"), &mut order, &mut number);

    let link = |s: &Slot| doc.resolve(*s).map_or("-".to_owned(), |t| number[&t].to_string());
    let text = |t: &Text| format!("{:?}{}", t.as_str(), if t.wide() { " (wide)" } else { "" });
    let value = |v: &Value| match v {
        Value::Obj(s) | Value::BareRef(s) => format!("-> {}", link(s)),
        Value::Named(s, t) => format!("-> {} named {}", link(s), text(t)),
        Value::List(l) => format!("[{}]", l.iter().map(link).collect::<Vec<_>>().join(" ")),
        Value::Map(m) => {
            format!("{{{}}}", m.iter().map(|(k, s)| format!("{}: {}", text(k), link(s))).collect::<Vec<_>>().join(", "))
        }
        Value::Str(t) => text(t),
        Value::Strings(l) => format!("[{}]", l.iter().map(text).collect::<Vec<_>>().join(", ")),
        // Floats by their bits, so a NaN matches itself.
        Value::F32(x) => format!("f32 {:08x}", x.to_bits()),
        Value::F64(x) => format!("f64 {:016x}", x.to_bits()),
        Value::Floats(l) => format!("f32s {:08x?}", l.iter().map(|x| x.to_bits()).collect::<Vec<_>>()),
        Value::Doubles(l) => format!("f64s {:016x?}", l.iter().map(|x| x.to_bits()).collect::<Vec<_>>()),
        Value::Color(c) => format!("colour {:08x?}", c.map(f32::to_bits)),
        other => format!("{other:?}"),
    };
    order
        .iter()
        .enumerate()
        .map(|(i, &o)| {
            let obj = doc.obj(o);
            let fields: Vec<String> = obj.fields.iter().map(|f| format!("{:x}: {}", f.id, value(&f.value))).collect();
            format!("#{i} class {:x} {{{}}}", obj.class, fields.join(", "))
        })
        .collect()
}

/// Rule 8: no field that holds one object holds a "class 1" reference.
fn no_class_1_reference_in_a_field(doc: &Document) -> bool {
    doc.objects.iter().flat_map(|o| &o.fields).all(|f| !matches!(f.value, Value::Obj(Slot::Ref(_))))
}

/// Where two files first differ, to say so when a round trip fails.
fn first_difference(a: &[u8], b: &[u8]) -> String {
    match a.iter().zip(b).position(|(x, y)| x != y) {
        Some(at) => format!("first difference at byte 0x{at:x} of 0x{:x} and 0x{:x}", a.len(), b.len()),
        None => format!("same up to the shorter: 0x{:x} and 0x{:x} bytes", a.len(), b.len()),
    }
}

/// Writes a document out, reads it back and checks the file is trusted, the tree is the same, rule 8 holds and the
/// file writes back byte for byte. Returns the file.
fn written_and_read_back(doc: &Document) -> Result<Vec<u8>, TestCaseError> {
    let file = bitwig::encode(doc, None).map_err(|e| TestCaseError::fail(format!("not written: {e}")))?;
    let back = bitwig::decode(file.clone()).map_err(|_| TestCaseError::fail("not Bitwig's format"))?;
    prop_assert_eq!(&back.body_error, &None, "the written file isn't read to the zip");
    prop_assert_eq!(tree(&back), tree(doc), "the tree read back isn't the tree written");
    prop_assert!(no_class_1_reference_in_a_field(&back), "a class 1 reference in a single-object field");
    let again = bitwig::encode(&back, None).map_err(|e| TestCaseError::fail(format!("not written again: {e}")))?;
    prop_assert!(again == file, "written again differently: {}", first_difference(&again, &file));
    Ok(file)
}

/// Random numbers from a seed, for the check below (the same each run).
fn seeded(seed: u64, n: usize) -> Vec<u32> {
    let mut x = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 32) as u32
        })
        .collect()
}

/// The random files are worth testing on: big enough, with every kind of link, so a round trip that passes isn't
/// passing on files that are nearly empty.
#[test]
fn the_random_files_hold_every_kind_of_link() {
    let (mut most, mut refs, mut bare, mut zips, mut meta, mut wide) = (0, 0, 0, 0, 0, 0);
    for seed in 0..200 {
        let doc = bitwig::decode(random_file(&seeded(seed, 3000))).expect("Bitwig's format");
        assert_eq!(doc.body_error, None);
        most = most.max(doc.objects.len());
        zips += usize::from(doc.has_zip);
        meta += doc.meta.len();
        for f in doc.objects.iter().flat_map(|o| &o.fields) {
            refs += f.value.targets().filter(|s| matches!(s, Slot::Ref(_))).count();
            bare += usize::from(matches!(f.value, Value::BareRef(_)));
            wide += usize::from(matches!(&f.value, Value::Str(t) if t.wide()));
        }
    }
    assert!(most >= 30, "the biggest file has only {most} objects");
    for (what, n) in
        [("references", refs), ("bare references", bare), ("zips", zips), ("metadata", meta), ("UTF-16 texts", wide)]
    {
        assert!(n >= 20, "only {n} {what} in 200 files");
    }
}

fn numbers() -> impl Strategy<Value = Vec<u32>> {
    prop::collection::vec(any::<u32>(), 0..3000)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

    #[test]
    fn a_random_file_writes_back_byte_for_byte(numbers in numbers()) {
        let raw = random_file(&numbers);
        let doc = bitwig::decode(raw.clone()).map_err(|_| TestCaseError::fail("not Bitwig's format"))?;
        prop_assert_eq!(&doc.body_error, &None, "not read to the zip");
        prop_assert!(no_class_1_reference_in_a_field(&doc));
        let again = bitwig::encode(&doc, None).map_err(|e| TestCaseError::fail(format!("not written: {e}")))?;
        prop_assert!(again == raw, "written back differently: {}", first_difference(&again, &raw));
    }

    #[test]
    fn a_random_tree_writes_out_and_reads_back_the_same(numbers in numbers()) {
        written_and_read_back(&random_tree(&numbers))?;
    }

    #[test]
    fn a_random_file_edited_reads_back_as_edited(file in numbers(), edit in prop::collection::vec(any::<u32>(), 1..4)) {
        let mut doc = bitwig::decode(random_file(&file)).map_err(|_| TestCaseError::fail("not Bitwig's format"))?;
        // Take out one field, or one entry of a list or map, from an object the root reaches. What it held may be
        // where another object was defined, so a later reference to it must now define it instead.
        let mut c = Choices::new(&edit);
        let mut reached = Vec::new();
        let mut todo = vec![doc.root.expect("a root")];
        while let Some(o) = todo.pop() {
            if !reached.contains(&o) {
                reached.push(o);
                todo.extend(doc.obj(o).fields.iter().flat_map(|f| f.value.targets()).filter_map(|s| doc.resolve(s)));
            }
        }
        let o = reached[c.pick(reached.len() as u32) as usize];
        let fields = &mut doc.obj_mut(o).fields;
        if !fields.is_empty() {
            let f = c.pick(fields.len() as u32) as usize;
            match &mut fields[f].value {
                Value::List(l) if !l.is_empty() => {
                    l.remove(c.pick(l.len() as u32) as usize);
                }
                Value::Map(m) if !m.is_empty() => {
                    m.remove(c.pick(m.len() as u32) as usize);
                }
                _ => {
                    fields.remove(f);
                }
            }
        }
        written_and_read_back(&doc)?;
    }

    #[test]
    fn a_damaged_random_file_never_stops_the_reader(
        numbers in numbers(),
        cut in any::<u32>(),
        changes in prop::collection::vec((any::<u32>(), any::<u8>()), 0..4),
    ) {
        let mut raw = random_file(&numbers);
        for (at, byte) in changes {
            let at = at as usize % raw.len();
            raw[at] = byte;
        }
        if cut % 3 == 0 {
            raw.truncate(cut as usize % (raw.len() + 1));
        }
        // It may not be Bitwig's any more, or not be read to the zip, but reading it must never panic. One that's
        // still read cleanly and can be written must write a file that reads back the same.
        if let Ok(doc) = bitwig::decode(raw)
            && doc.body_error.is_none()
            && bitwig::encode(&doc, None).is_ok()
        {
            written_and_read_back(&doc)?;
        }
    }
}
