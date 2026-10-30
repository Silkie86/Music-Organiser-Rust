//! Bitwig's "BtWg" binary format (`.bwproject`; `.bwpreset` and `.bwclip` share it), worked out from projects saved
//! by Bitwig 5.1, 5.2 and 6.1. A decode is only trusted when it ends exactly where the header says the embedded zip
//! starts (rule 8).
//!
//! Layout:
//! - `BtWg` + 38 ASCII hex characters: 4 + 4 + 4 (format), 8 (body offset), 16 (zip offset), "00".
//! - Metadata: u32 4, the string "meta", then entries of (u32 1, string key, u8 type, value) until u32 0.
//! - The body, at the body offset: one tree of objects.
//! - A zip, at the zip offset: `plugin-states/<id>.vstpreset|.fxb|.fxp` and other attachments.
//!
//! Everything is big-endian. An object is a u32 class followed by fields of (u32 field id, u8 type, value) and ends
//! with field id 0. Where an object is expected, class 0 means none and class 1 means "the object numbered N" (u32 N
//! follows; objects are numbered from 1 in the order they're written). Strings are a u32 length then UTF-8; when the
//! length's top bit is set, the other bits count UTF-16BE characters.

use std::collections::HashMap;
use std::sync::Arc;

/// An object's place in [`Document::objects`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjId(pub u32);

/// Where an object goes in a field, list or map.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    /// No object (class 0).
    Null,
    /// An object: written in full where it's first reached, and as a reference after that.
    Obj(ObjId),
    /// A reference as the file wrote it: the object numbered N, from 1. Kept as it was, so an untouched document is
    /// written back byte for byte.
    Ref(u32),
}

/// Text as Bitwig wrote it. `wide` is a string written as UTF-16; `raw` keeps the bytes of one that wasn't valid
/// text, so it's written back as it was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Text {
    pub text: Arc<str>,
    pub wide: bool,
    pub raw: Option<Arc<[u8]>>,
}

impl Text {
    /// Text made in code: written as UTF-8 when it's plain ASCII, else as UTF-16, as Bitwig writes it.
    pub fn new(text: &str) -> Text {
        Text { text: text.into(), wide: !text.is_ascii(), raw: None }
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }
}

/// An opaque run of bytes (type 0x0d): where it is in the file, or bytes made in code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Blob {
    At { offset: usize, len: usize },
    Bytes(Vec<u8>),
}

/// A field's value. The name says what the type byte is (in brackets).
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// 0x01.
    U8(u8),
    /// 0x05: a switch, 0 or 1.
    Bool(u8),
    /// 0x02.
    I16(i16),
    /// 0x03.
    I32(i32),
    /// 0x04.
    I64(i64),
    /// 0x06.
    F32(f32),
    /// 0x07.
    F64(f64),
    /// 0x08.
    Str(Text),
    /// 0x09: an object, a reference, or none.
    Obj(Slot),
    /// 0x0a.
    Null,
    /// 0x0b: a bare reference, the object's number with no class before it.
    BareRef(Slot),
    /// 0x0d.
    Blob(Blob),
    /// 0x0f: u32 count, then i32s.
    Ints(Vec<i32>),
    /// 0x12: objects until the marker 3.
    List(Vec<Slot>),
    /// 0x14: (u8 1, string key, object) per entry, then u8 0.
    Map(Vec<(Text, Slot)>),
    /// 0x15: 16 bytes, an id.
    Uuid([u8; 16]),
    /// 0x16: four floats, a colour.
    Color([f32; 4]),
    /// 0x17: u32 count, then f32s (FM-4's modulation matrix).
    Floats(Vec<f32>),
    /// 0x18: u32 count, then f64s (Bitwig 6.1 keeps a Sampler sample's analysis this way).
    Doubles(Vec<f64>),
    /// 0x19: u32 count, then strings.
    Strings(Vec<Text>),
    /// 0x1a: an object (or reference) followed by a string.
    Named(Slot, Text),
}

impl Value {
    /// The type byte a value is written with when nothing says otherwise.
    pub fn type_byte(&self) -> u8 {
        match self {
            Value::U8(_) => 0x01,
            Value::I16(_) => 0x02,
            Value::I32(_) => 0x03,
            Value::I64(_) => 0x04,
            Value::Bool(_) => 0x05,
            Value::F32(_) => 0x06,
            Value::F64(_) => 0x07,
            Value::Str(_) => 0x08,
            Value::Obj(_) => 0x09,
            Value::Null => 0x0a,
            Value::BareRef(_) => 0x0b,
            Value::Blob(_) => 0x0d,
            Value::Ints(_) => 0x0f,
            Value::List(_) => 0x12,
            Value::Map(_) => 0x14,
            Value::Uuid(_) => 0x15,
            Value::Color(_) => 0x16,
            Value::Floats(_) => 0x17,
            Value::Doubles(_) => 0x18,
            Value::Strings(_) => 0x19,
            Value::Named(..) => 0x1a,
        }
    }

    /// The object slots in the value, in the order they're written. A bare reference isn't one.
    pub fn slots(&self) -> Vec<Slot> {
        match self {
            Value::Obj(s) => vec![*s],
            Value::List(l) => l.clone(),
            Value::Map(m) => m.iter().map(|(_, s)| *s).collect(),
            Value::Named(s, _) => vec![*s],
            _ => Vec::new(),
        }
    }
}

/// One field of an object.
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub id: u32,
    pub value: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Object {
    /// The object's kind. A few kinds are laid out the same, such as a VST2 and a VST3 plugin device.
    pub class: u32,
    pub fields: Vec<Field>,
    /// The object it was read inside, and in which field; none for the root and for objects made in code.
    pub parent: Option<(ObjId, u32)>,
}

impl Object {
    pub fn new(class: u32) -> Object {
        Object { class, fields: Vec::new(), parent: None }
    }

    /// The first field with this id.
    pub fn get(&self, id: u32) -> Option<&Value> {
        self.fields.iter().find(|f| f.id == id).map(|f| &f.value)
    }

    pub fn has(&self, id: u32) -> bool {
        self.fields.iter().any(|f| f.id == id)
    }

    /// Changes a field's value, or adds the field at the end.
    pub fn set(&mut self, id: u32, value: Value) {
        match self.fields.iter_mut().find(|f| f.id == id) {
            Some(f) => f.value = value,
            None => self.fields.push(Field { id, value }),
        }
    }

    /// A text field, or "".
    pub fn str(&self, id: u32) -> &str {
        match self.get(id) {
            Some(Value::Str(t)) => t.as_str(),
            _ => "",
        }
    }

    /// A number field as a float, whatever its type.
    pub fn num(&self, id: u32) -> Option<f64> {
        match self.get(id)? {
            Value::F64(v) => Some(*v),
            Value::F32(v) => Some(*v as f64),
            Value::I32(v) => Some(*v as f64),
            Value::I16(v) => Some(*v as f64),
            Value::I64(v) => Some(*v as f64),
            Value::U8(v) | Value::Bool(v) => Some(*v as f64),
            _ => None,
        }
    }
}

/// A decoded file.
#[derive(Clone, Debug)]
pub struct Document {
    /// The file as it was read.
    pub raw: Arc<[u8]>,
    /// The 38 characters after "BtWg".
    pub header: [u8; 38],
    /// The metadata, in file order.
    pub meta: Vec<(Text, Value)>,
    /// Where the metadata ends; the body follows, sometimes after padding.
    pub meta_end: usize,
    pub root: Option<ObjId>,
    /// Every object, in the order the file has them (a reference's number is its place here plus one), then any made
    /// in code.
    pub objects: Vec<Object>,
    pub body_offset: usize,
    pub zip_offset: usize,
    /// Why the body couldn't be decoded, or none when it decoded cleanly up to the zip.
    pub body_error: Option<String>,
}

pub const HEADER: usize = 42;

/// A file in Bitwig's format, by its first bytes.
pub fn is_bitwig(raw: &[u8]) -> bool {
    raw.len() >= HEADER && raw.starts_with(b"BtWg")
}

/// Why a file isn't Bitwig's at all. (A file that is, but whose body can't be read, decodes with
/// [`Document::body_error`] saying why.)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotBitwig;

impl Document {
    /// The object with this id.
    pub fn obj(&self, id: ObjId) -> &Object {
        &self.objects[id.0 as usize]
    }

    pub fn obj_mut(&mut self, id: ObjId) -> &mut Object {
        &mut self.objects[id.0 as usize]
    }

    /// Adds an object made in code; it's written where a field first reaches it.
    pub fn add(&mut self, object: Object) -> ObjId {
        self.objects.push(object);
        ObjId(self.objects.len() as u32 - 1)
    }

    /// The object a slot holds or refers to.
    pub fn resolve(&self, slot: Slot) -> Option<ObjId> {
        match slot {
            Slot::Null => None,
            Slot::Obj(id) => Some(id),
            Slot::Ref(n) => (n >= 1 && (n as usize) <= self.objects.len()).then(|| ObjId(n - 1)),
        }
    }

    /// The object in a field (held, referred to, or named).
    pub fn field_obj(&self, of: ObjId, id: u32) -> Option<ObjId> {
        match self.obj(of).get(id)? {
            Value::Obj(s) | Value::BareRef(s) | Value::Named(s, _) => self.resolve(*s),
            _ => None,
        }
    }

    /// The objects in a list field.
    pub fn field_list(&self, of: ObjId, id: u32) -> Vec<ObjId> {
        match self.obj(of).get(id) {
            Some(Value::List(l)) => l.iter().filter_map(|s| self.resolve(*s)).collect(),
            _ => Vec::new(),
        }
    }

    /// The bytes of a blob.
    pub fn blob<'a>(&'a self, b: &'a Blob) -> &'a [u8] {
        match b {
            Blob::At { offset, len } => &self.raw[*offset..*offset + *len],
            Blob::Bytes(v) => v,
        }
    }

    /// A metadata value by key.
    pub fn meta(&self, key: &str) -> Option<&Value> {
        self.meta.iter().find(|(k, _)| k.as_str() == key).map(|(_, v)| v)
    }

    /// A metadata text, or "".
    pub fn meta_str(&self, key: &str) -> &str {
        match self.meta(key) {
            Some(Value::Str(t)) => t.as_str(),
            _ => "",
        }
    }

    /// A metadata list of texts.
    pub fn meta_strings(&self, key: &str) -> Vec<&str> {
        match self.meta(key) {
            Some(Value::Strings(l)) => l.iter().map(Text::as_str).collect(),
            _ => Vec::new(),
        }
    }

    /// The embedded zip's bytes (plugin states), when there is one.
    pub fn zip(&self) -> Option<&[u8]> {
        let z = self.raw.get(self.zip_offset..)?;
        (self.zip_offset > 0 && z.len() >= 4 && z.starts_with(b"PK")).then_some(z)
    }

    /// The object a parent field holds it in, walking up: the first one that `matches`.
    pub fn ancestor(&self, of: ObjId, matches: impl Fn(&Object) -> bool) -> Option<ObjId> {
        let mut at = self.obj(of).parent;
        while let Some((p, _)) = at {
            if matches(self.obj(p)) {
                return Some(p);
            }
            at = self.obj(p).parent;
        }
        None
    }
}

/// Decodes a file. A file that isn't Bitwig's is an error; one that is but whose body can't be read is a document
/// with [`Document::body_error`] set (and the metadata as far as it went).
pub fn decode(raw: impl Into<Arc<[u8]>>) -> Result<Document, NotBitwig> {
    let raw: Arc<[u8]> = raw.into();
    if !is_bitwig(&raw) {
        return Err(NotBitwig);
    }
    let mut header = [0u8; 38];
    header.copy_from_slice(&raw[4..HEADER]);
    let hex = |range: std::ops::Range<usize>| {
        std::str::from_utf8(&header[range]).ok().and_then(|s| u64::from_str_radix(s, 16).ok()).unwrap_or(0)
    };
    let body = hex(12..20) as usize;
    let zip = match hex(20..36) as usize {
        z if z == 0 || z > raw.len() => raw.len(),
        z => z,
    };
    let mut doc = Document {
        raw: raw.clone(),
        header,
        meta: Vec::new(),
        meta_end: HEADER,
        root: None,
        objects: Vec::new(),
        body_offset: body,
        zip_offset: zip,
        body_error: None,
    };
    let mut r = Reader { d: &raw, objects: Vec::new(), strings: HashMap::new(), depth: 0 };
    match r.meta(HEADER) {
        Ok((meta, end)) => {
            doc.meta = meta;
            doc.meta_end = end;
        }
        Err(e) => {
            doc.objects = r.objects;
            doc.body_error = Some(format!("metadata: {e}"));
            return Ok(doc);
        }
    }
    if body <= HEADER || body >= zip {
        doc.objects = r.objects;
        doc.body_error = Some("header has no body offset".into());
        return Ok(doc);
    }
    let mut pos = body;
    match r.object(&mut pos) {
        Ok(root) => {
            doc.root = root;
            if pos != zip {
                doc.body_error = Some(format!("body ended at 0x{pos:x}, expected 0x{zip:x}"));
            }
        }
        Err(e) => doc.body_error = Some(e),
    }
    doc.objects = r.objects;
    Ok(doc)
}

/// Objects nested deeper than this are taken as a damaged file rather than followed.
const MAX_DEPTH: usize = 512;
/// Strings up to this long are shared: a project uses the same few names thousands of times.
const SHORT_STRING: usize = 64;

struct Reader<'a> {
    d: &'a [u8],
    objects: Vec<Object>,
    strings: HashMap<&'a [u8], Arc<str>>,
    depth: usize,
}

type R<T> = Result<T, String>;

const PAST_END: &str = "runs past end of file";

impl<'a> Reader<'a> {
    fn at(&self, p: usize, n: usize) -> R<&'a [u8]> {
        self.d.get(p..p.checked_add(n).ok_or(PAST_END)?).ok_or_else(|| PAST_END.to_owned())
    }

    /// `n` bytes at `p`, moving `p` past them.
    fn take(&self, p: &mut usize, n: usize) -> R<&'a [u8]> {
        let b = self.at(*p, n)?;
        *p += n;
        Ok(b)
    }

    fn byte(&self, p: usize) -> R<u8> {
        self.d.get(p).copied().ok_or_else(|| PAST_END.to_owned())
    }

    fn u32(&self, p: usize) -> R<u32> {
        Ok(u32::from_be_bytes(self.at(p, 4)?.try_into().expect("4 bytes")))
    }

    fn text(&mut self, p: &mut usize) -> R<Text> {
        let n = self.u32(*p)?;
        *p += 4;
        if n & 0x8000_0000 != 0 {
            let chars = (n & 0x7FFF_FFFF) as usize;
            let bytes = self.at(*p, chars.checked_mul(2).ok_or(PAST_END)?)?;
            *p += bytes.len();
            let units: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes(*c)).collect();
            return Ok(match String::from_utf16(&units) {
                Ok(s) => Text { text: s.into(), wide: true, raw: None },
                Err(_) => Text { text: String::from_utf16_lossy(&units).into(), wide: true, raw: Some(bytes.into()) },
            });
        }
        let bytes = self.at(*p, n as usize)?;
        *p += bytes.len();
        if bytes.len() <= SHORT_STRING
            && let Some(s) = self.strings.get(bytes)
        {
            return Ok(Text { text: s.clone(), wide: false, raw: None });
        }
        Ok(match std::str::from_utf8(bytes) {
            Ok(s) => {
                let s: Arc<str> = s.into();
                if bytes.len() <= SHORT_STRING {
                    self.strings.insert(bytes, s.clone());
                }
                Text { text: s, wide: false, raw: None }
            }
            Err(_) => Text { text: String::from_utf8_lossy(bytes).into(), wide: false, raw: Some(bytes.into()) },
        })
    }

    /// The metadata from `p`, and where it ends. A file without the "meta" block has none.
    fn meta(&mut self, mut p: usize) -> R<(Vec<(Text, Value)>, usize)> {
        let mut meta = Vec::new();
        if self.u32(p)? != 4 {
            return Ok((meta, p));
        }
        let start = p;
        p += 4;
        if self.text(&mut p)?.as_str() != "meta" {
            return Ok((meta, start));
        }
        while self.u32(p)? == 1 {
            p += 4;
            let key = self.text(&mut p)?;
            let ty = self.byte(p)?;
            p += 1;
            let value = self.value(&mut p, ty, None)?;
            meta.push((key, value));
        }
        Ok((meta, p))
    }

    /// An object (or none, for class 0) at `p`.
    fn object(&mut self, p: &mut usize) -> R<Option<ObjId>> {
        let class = self.u32(*p)?;
        *p += 4;
        match class {
            0 => return Ok(None),
            1 => return Err("unexpected reference".into()),
            _ => {}
        }
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err("objects nested too deeply".into());
        }
        let id = ObjId(self.objects.len() as u32);
        self.objects.push(Object::new(class));
        let mut fields = Vec::new();
        loop {
            let fid = self.u32(*p)?;
            *p += 4;
            if fid == 0 {
                break;
            }
            let ty = self.byte(*p)?;
            *p += 1;
            let value = self.value(p, ty, Some((id, fid)))?;
            fields.push(Field { id: fid, value });
        }
        self.objects[id.0 as usize].fields = fields;
        self.depth -= 1;
        Ok(Some(id))
    }

    /// An object slot: none, a reference, or an object written here. `parent` is the object and field it's in.
    fn slot(&mut self, p: &mut usize, parent: Option<(ObjId, u32)>) -> R<Slot> {
        if self.u32(*p)? == 1 {
            let n = self.u32(*p + 4)?;
            *p += 8;
            return Ok(Slot::Ref(n));
        }
        Ok(match self.object(p)? {
            Some(id) => {
                self.objects[id.0 as usize].parent = parent;
                Slot::Obj(id)
            }
            None => Slot::Null,
        })
    }

    /// A count of `size`-byte items that must fit in the file.
    fn count(&self, p: usize, size: usize, what: &str) -> R<usize> {
        let n = self.u32(p)? as usize;
        if n.checked_mul(size).is_none_or(|bytes| p + 4 + bytes > self.d.len()) {
            return Err(format!("{what} runs past end of file"));
        }
        Ok(n)
    }

    fn value(&mut self, p: &mut usize, ty: u8, parent: Option<(ObjId, u32)>) -> R<Value> {
        Ok(match ty {
            0x01 => Value::U8(self.take(p, 1)?[0]),
            0x05 => Value::Bool(self.take(p, 1)?[0]),
            0x02 => Value::I16(i16::from_be_bytes(self.take(p, 2)?.try_into().expect("2"))),
            0x03 => Value::I32(i32::from_be_bytes(self.take(p, 4)?.try_into().expect("4"))),
            0x04 => Value::I64(i64::from_be_bytes(self.take(p, 8)?.try_into().expect("8"))),
            0x06 => Value::F32(f32::from_be_bytes(self.take(p, 4)?.try_into().expect("4"))),
            0x07 => Value::F64(f64::from_be_bytes(self.take(p, 8)?.try_into().expect("8"))),
            0x08 => Value::Str(self.text(p)?),
            0x09 => Value::Obj(self.slot(p, parent)?),
            0x0a => Value::Null,
            0x0b => Value::BareRef(Slot::Ref(u32::from_be_bytes(self.take(p, 4)?.try_into().expect("4")))),
            0x0d => {
                let n = self.count(*p, 1, "blob")?;
                let blob = Blob::At { offset: *p + 4, len: n };
                *p += 4 + n;
                Value::Blob(blob)
            }
            0x0f => {
                let n = self.count(*p, 4, "int array")?;
                let a = self.at(*p + 4, 4 * n)?.as_chunks::<4>().0.iter().map(|c| i32::from_be_bytes(*c)).collect();
                *p += 4 + 4 * n;
                Value::Ints(a)
            }
            0x12 => {
                let mut list = Vec::new();
                while self.u32(*p)? != 3 {
                    list.push(self.slot(p, parent)?);
                }
                *p += 4;
                Value::List(list)
            }
            0x14 => {
                let mut map = Vec::new();
                while self.byte(*p)? == 1 {
                    *p += 1;
                    let key = self.text(p)?;
                    map.push((key, self.slot(p, parent)?));
                }
                if self.byte(*p)? != 0 {
                    return Err(format!("bad map end at 0x{:x}", *p));
                }
                *p += 1;
                Value::Map(map)
            }
            0x15 => Value::Uuid(self.take(p, 16)?.try_into().expect("16")),
            0x16 => {
                let b = self.take(p, 16)?;
                Value::Color(std::array::from_fn(|i| f32::from_be_bytes(b[4 * i..4 * i + 4].try_into().expect("4"))))
            }
            0x17 => {
                let n = self.count(*p, 4, "float list")?;
                let f = self.at(*p + 4, 4 * n)?.as_chunks::<4>().0.iter().map(|c| f32::from_be_bytes(*c)).collect();
                *p += 4 + 4 * n;
                Value::Floats(f)
            }
            0x18 => {
                let n = self.count(*p, 8, "double list")?;
                let f = self.at(*p + 4, 8 * n)?.as_chunks::<8>().0.iter().map(|c| f64::from_be_bytes(*c)).collect();
                *p += 4 + 8 * n;
                Value::Doubles(f)
            }
            0x19 => {
                // Each string takes at least its 4-byte length.
                let n = self.count(*p, 4, "string list")?;
                *p += 4;
                let mut list = Vec::with_capacity(n);
                for _ in 0..n {
                    list.push(self.text(p)?);
                }
                Value::Strings(list)
            }
            0x1a => {
                let slot = self.slot(p, parent)?;
                Value::Named(slot, self.text(p)?)
            }
            _ => return Err(format!("unknown value type 0x{ty:02x} at 0x{:x}", *p - 1)),
        })
    }
}
