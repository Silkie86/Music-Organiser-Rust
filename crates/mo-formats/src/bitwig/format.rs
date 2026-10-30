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

/// Text as Bitwig wrote it: UTF-8, or UTF-16 ("wide"). One pointer, shared by every field with the same short text,
/// since a big project holds millions of fields.
#[derive(Clone, PartialEq, Eq)]
pub struct Text(Arc<TextInner>);

#[derive(PartialEq, Eq)]
struct TextInner {
    text: Box<str>,
    wide: bool,
    /// The bytes of text that wasn't valid UTF-8 or UTF-16, so it's written back as it was.
    raw: Option<Box<[u8]>>,
}

impl Text {
    /// Text made in code: written as UTF-8 when it's plain ASCII, else as UTF-16, as Bitwig writes it.
    pub fn new(text: &str) -> Text {
        Text(Arc::new(TextInner { text: text.into(), wide: !text.is_ascii(), raw: None }))
    }

    fn read(text: String, wide: bool, raw: Option<&[u8]>) -> Text {
        Text(Arc::new(TextInner { text: text.into(), wide, raw: raw.map(Into::into) }))
    }

    pub fn as_str(&self) -> &str {
        &self.0.text
    }

    /// Written as UTF-16.
    pub fn wide(&self) -> bool {
        self.0.wide
    }

    /// The bytes as the file had them, for text that wasn't valid.
    pub(crate) fn raw(&self) -> Option<&[u8]> {
        self.0.raw.as_deref()
    }
}

impl std::fmt::Debug for Text {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.as_str())
    }
}

/// An opaque run of bytes (type 0x0d). It shares the file it came from rather than copying it, and goes with the
/// value when the value is copied into another document.
#[derive(Clone, PartialEq, Eq)]
pub struct Blob {
    data: Arc<[u8]>,
    start: usize,
    len: usize,
}

impl Blob {
    /// Bytes made in code.
    pub fn new(bytes: &[u8]) -> Blob {
        Blob { data: bytes.into(), start: 0, len: bytes.len() }
    }

    pub fn bytes(&self) -> &[u8] {
        &self.data[self.start..self.start + self.len]
    }
}

impl std::fmt::Debug for Blob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Blob({} bytes)", self.len)
    }
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
    Blob(Box<Blob>),
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

    /// Every object the value points at, in the order they're written: what it holds, lists, maps or names, and a
    /// bare reference's object.
    pub fn targets(&self) -> Targets<'_> {
        match self {
            Value::Obj(s) | Value::BareRef(s) | Value::Named(s, _) => Targets::One(Some(*s)),
            Value::List(l) => Targets::List(l.iter()),
            Value::Map(m) => Targets::Map(m.iter()),
            _ => Targets::One(None),
        }
    }

    /// A number as the same kind of number as `like` (a float as a float, a switch as a switch), so a field changed in
    /// code keeps the type Bitwig wrote it with. Anything else is left as it is.
    pub fn same_kind_as(self, like: &Value) -> Value {
        let n = match self {
            Value::U8(v) | Value::Bool(v) => v as f64,
            Value::I16(v) => v as f64,
            Value::I32(v) => v as f64,
            Value::I64(v) => v as f64,
            Value::F32(v) => v as f64,
            Value::F64(v) => v,
            v => return v,
        };
        match like {
            Value::U8(_) => Value::U8(n as u8),
            Value::Bool(_) => Value::Bool(n as u8),
            Value::I16(_) => Value::I16(n as i16),
            Value::I32(_) => Value::I32(n as i32),
            Value::I64(_) => Value::I64(n as i64),
            Value::F32(_) => Value::F32(n as f32),
            Value::F64(_) => Value::F64(n),
            _ => self,
        }
    }
}

/// The objects a value points at (see [`Value::targets`]).
pub enum Targets<'a> {
    One(Option<Slot>),
    List(std::slice::Iter<'a, Slot>),
    Map(std::slice::Iter<'a, (Text, Slot)>),
}

impl Iterator for Targets<'_> {
    type Item = Slot;

    fn next(&mut self) -> Option<Slot> {
        match self {
            Targets::One(s) => s.take(),
            Targets::List(l) => l.next().copied(),
            Targets::Map(m) => m.next().map(|(_, s)| *s),
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

    /// Changes a field's value, or adds the field at the end. A number keeps the kind of number the field had.
    pub fn set(&mut self, id: u32, value: Value) {
        match self.fields.iter_mut().find(|f| f.id == id) {
            Some(f) => f.value = value.same_kind_as(&f.value),
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
    /// Where the zip starts: where the header says, or the end of the file when it says none.
    pub zip_offset: usize,
    /// The header says there's a zip.
    pub has_zip: bool,
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
    pub fn ancestor(&self, of: ObjId, matches: impl Fn(ObjId, &Object) -> bool) -> Option<ObjId> {
        let mut at = self.obj(of).parent;
        // No more steps than there are objects, whatever objects made in code say about their parents.
        for _ in 0..self.objects.len() {
            let (p, _) = at?;
            if matches(p, self.obj(p)) {
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
    let in_header = hex(20..36) as usize;
    let zip = if in_header == 0 || in_header > raw.len() { raw.len() } else { in_header };
    let mut doc = Document {
        raw: raw.clone(),
        header,
        meta: Vec::new(),
        meta_end: HEADER,
        root: None,
        objects: Vec::new(),
        body_offset: body,
        zip_offset: zip,
        has_zip: in_header > 0,
        body_error: None,
    };
    let mut r = Reader { d: &raw, file: &raw, objects: Vec::new(), strings: HashMap::new(), depth: 0 };
    match r.meta(HEADER, &mut doc.meta) {
        Ok(end) => doc.meta_end = end,
        Err(e) => {
            // The metadata as far as it went stays.
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
    /// The same bytes, shared with each blob.
    file: &'a Arc<[u8]>,
    objects: Vec<Object>,
    strings: HashMap<&'a [u8], Text>,
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

    /// `N` bytes at `p`, moving `p` past them.
    fn take_n<const N: usize>(&self, p: &mut usize) -> R<[u8; N]> {
        Ok(*self.take(p, N)?.first_chunk().ok_or(PAST_END)?)
    }

    /// The byte at `p`, moving `p` past it.
    fn byte_at(&self, p: &mut usize) -> R<u8> {
        let [b] = self.take_n(p)?;
        Ok(b)
    }

    fn byte(&self, p: usize) -> R<u8> {
        self.d.get(p).copied().ok_or_else(|| PAST_END.to_owned())
    }

    fn u32(&self, p: usize) -> R<u32> {
        Ok(u32::from_be_bytes(*self.at(p, 4)?.first_chunk().ok_or(PAST_END)?))
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
                Ok(s) => Text::read(s, true, None),
                Err(_) => Text::read(String::from_utf16_lossy(&units), true, Some(bytes)),
            });
        }
        let bytes = self.at(*p, n as usize)?;
        *p += bytes.len();
        if let Some(t) = self.strings.get(bytes) {
            return Ok(t.clone());
        }
        let t = match std::str::from_utf8(bytes) {
            Ok(s) => Text::read(s.to_owned(), false, None),
            Err(_) => Text::read(String::from_utf8_lossy(bytes).into_owned(), false, Some(bytes)),
        };
        if bytes.len() <= SHORT_STRING {
            self.strings.insert(bytes, t.clone());
        }
        Ok(t)
    }

    /// The metadata from `p` into `meta`, and where it ends. A file without the "meta" block has none.
    fn meta(&mut self, mut p: usize, meta: &mut Vec<(Text, Value)>) -> R<usize> {
        if self.u32(p)? != 4 {
            return Ok(p);
        }
        let start = p;
        p += 4;
        if self.text(&mut p)?.as_str() != "meta" {
            return Ok(start);
        }
        while self.u32(p)? == 1 {
            p += 4;
            let key = self.text(&mut p)?;
            let ty = self.byte(p)?;
            p += 1;
            let value = self.value(&mut p, ty, None)?;
            meta.push((key, value));
        }
        Ok(p)
    }

    /// The root object (or none, for class 0) at `p`.
    fn object(&mut self, p: &mut usize) -> R<Option<ObjId>> {
        if self.u32(*p)? == 1 {
            return Err("unexpected reference".into());
        }
        match self.value(p, 0x09, None)? {
            Value::Obj(Slot::Obj(id)) => Ok(Some(id)),
            _ => Ok(None),
        }
    }

    /// Starts an object slot at `p`: none or a reference is done at once; an object written here is started on the
    /// stack (it's done when its fields end). `parent` is the object and field it's in.
    fn start_slot(&mut self, p: &mut usize, parent: Option<(ObjId, u32)>, stack: &mut Vec<Frame>) -> R<Option<Slot>> {
        match self.u32(*p)? {
            0 => {
                *p += 4;
                Ok(Some(Slot::Null))
            }
            1 => {
                let n = self.reference(*p + 4)?;
                *p += 8;
                Ok(Some(Slot::Ref(n)))
            }
            class => {
                *p += 4;
                self.depth += 1;
                if self.depth > MAX_DEPTH {
                    return Err("objects nested too deeply".into());
                }
                let id = ObjId(self.objects.len() as u32);
                self.objects.push(Object { parent, ..Object::new(class) });
                stack.push(Frame::Obj { id, fields: Vec::new(), pending: 0 });
                Ok(None)
            }
        }
    }

    /// A reference's number: an object already read (Bitwig defines an object before it refers to it).
    fn reference(&self, p: usize) -> R<u32> {
        let n = self.u32(p)?;
        if n == 0 || n as usize > self.objects.len() {
            return Err(format!("reference at 0x{p:x} to object {n}, of {} read", self.objects.len()));
        }
        Ok(n)
    }

    /// A count of `size`-byte items that must fit in the file.
    fn count(&self, p: usize, size: usize, what: &str) -> R<usize> {
        let n = self.u32(p)? as usize;
        if n.checked_mul(size).is_none_or(|bytes| p + 4 + bytes > self.d.len()) {
            return Err(format!("{what} runs past end of file"));
        }
        Ok(n)
    }

    /// A value of type `ty` at `p`. Objects inside objects are followed with a stack of their own rather than by the
    /// function calling itself, so a damaged file nested deep can't run the thread out of stack.
    fn value(&mut self, p: &mut usize, ty: u8, parent: Option<(ObjId, u32)>) -> R<Value> {
        let bottom = match ty {
            0x09 => Frame::Single { parent, slot: None },
            0x12 => Frame::List { parent, items: Vec::new() },
            0x14 => Frame::Map { parent, items: Vec::new(), key: None },
            0x1a => Frame::Named { parent, slot: None },
            _ => return self.plain(p, ty),
        };
        let mut stack = vec![bottom];
        loop {
            let top = stack.len() - 1;
            // What the frame on top does next: wait for an object it started, or finish with a value.
            let done: Option<Value> = match &mut stack[top] {
                Frame::Obj { id, fields, pending } => {
                    let (id, fid) = (*id, self.u32(*p)?);
                    *p += 4;
                    if fid == 0 {
                        self.objects[id.0 as usize].fields = std::mem::take(fields);
                        self.depth -= 1;
                        stack.pop();
                        deliver(&mut stack, Slot::Obj(id));
                        continue;
                    }
                    let ty = self.byte(*p)?;
                    *p += 1;
                    let inside = Some((id, fid));
                    match ty {
                        0x09 => {
                            *pending = fid;
                            if let Some(slot) = self.start_slot(p, inside, &mut stack)? {
                                field(&mut stack[top], fid, Value::Obj(slot));
                            }
                        }
                        0x12 => stack.push(Frame::List { parent: inside, items: Vec::new() }),
                        0x14 => stack.push(Frame::Map { parent: inside, items: Vec::new(), key: None }),
                        0x1a => stack.push(Frame::Named { parent: inside, slot: None }),
                        _ => {
                            let value = self.plain(p, ty)?;
                            field(&mut stack[top], fid, value);
                        }
                    }
                    continue;
                }
                Frame::Single { parent, slot } => match *slot {
                    Some(s) => Some(Value::Obj(s)),
                    None => {
                        let parent = *parent;
                        if let Some(s) = self.start_slot(p, parent, &mut stack)? {
                            deliver(&mut stack, s);
                        }
                        continue;
                    }
                },
                Frame::List { parent, items } => {
                    if self.u32(*p)? == 3 {
                        *p += 4;
                        Some(Value::List(std::mem::take(items)))
                    } else {
                        let parent = *parent;
                        if let Some(s) = self.start_slot(p, parent, &mut stack)? {
                            deliver(&mut stack, s);
                        }
                        continue;
                    }
                }
                Frame::Map { parent, items, key } => match self.byte(*p)? {
                    1 => {
                        *p += 1;
                        *key = Some(self.text(p)?);
                        let parent = *parent;
                        if let Some(s) = self.start_slot(p, parent, &mut stack)? {
                            deliver(&mut stack, s);
                        }
                        continue;
                    }
                    0 => {
                        *p += 1;
                        Some(Value::Map(std::mem::take(items)))
                    }
                    _ => return Err(format!("bad map end at 0x{:x}", *p)),
                },
                Frame::Named { parent, slot } => match *slot {
                    Some(s) => Some(Value::Named(s, self.text(p)?)),
                    None => {
                        let parent = *parent;
                        if let Some(s) = self.start_slot(p, parent, &mut stack)? {
                            deliver(&mut stack, s);
                        }
                        continue;
                    }
                },
            };
            // Each frame above either goes on or finishes, so these are always there.
            let (Some(value), Some(finished)) = (done, stack.pop()) else {
                return Err(format!("lost its place at 0x{:x}", *p));
            };
            match stack.last_mut() {
                None => return Ok(value),
                Some(below) => {
                    let fid = match finished {
                        Frame::List { parent, .. } | Frame::Map { parent, .. } | Frame::Named { parent, .. } => {
                            parent.map_or(0, |(_, f)| f)
                        }
                        _ => 0,
                    };
                    field(below, fid, value);
                }
            }
        }
    }

    /// A value that holds no objects.
    #[inline(never)]
    fn plain(&mut self, p: &mut usize, ty: u8) -> R<Value> {
        Ok(match ty {
            0x01 => Value::U8(self.byte_at(p)?),
            0x05 => Value::Bool(self.byte_at(p)?),
            0x02 => Value::I16(i16::from_be_bytes(self.take_n(p)?)),
            0x03 => Value::I32(i32::from_be_bytes(self.take_n(p)?)),
            0x04 => Value::I64(i64::from_be_bytes(self.take_n(p)?)),
            0x06 => Value::F32(f32::from_be_bytes(self.take_n(p)?)),
            0x07 => Value::F64(f64::from_be_bytes(self.take_n(p)?)),
            0x08 => Value::Str(self.text(p)?),
            0x0a => Value::Null,
            0x0b => {
                let n = self.reference(*p)?;
                *p += 4;
                Value::BareRef(Slot::Ref(n))
            }
            0x0d => {
                let n = self.count(*p, 1, "blob")?;
                let blob = Blob { data: self.file.clone(), start: *p + 4, len: n };
                *p += 4 + n;
                Value::Blob(Box::new(blob))
            }
            0x0f => {
                let n = self.count(*p, 4, "int array")?;
                let a = self.at(*p + 4, 4 * n)?.as_chunks::<4>().0.iter().map(|c| i32::from_be_bytes(*c)).collect();
                *p += 4 + 4 * n;
                Value::Ints(a)
            }
            0x15 => Value::Uuid(self.take_n(p)?),
            0x16 => {
                let mut color = [0.0; 4];
                for c in &mut color {
                    *c = f32::from_be_bytes(self.take_n(p)?);
                }
                Value::Color(color)
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
            _ => return Err(format!("unknown value type 0x{ty:02x} at 0x{:x}", *p - 1)),
        })
    }
}

/// Something being read that holds objects, while the objects in it are read.
enum Frame {
    /// An object's fields; `pending` is the field whose object is being read.
    Obj {
        id: ObjId,
        fields: Vec<Field>,
        pending: u32,
    },
    /// A field holding one object (the value asked for).
    Single {
        parent: Option<(ObjId, u32)>,
        slot: Option<Slot>,
    },
    List {
        parent: Option<(ObjId, u32)>,
        items: Vec<Slot>,
    },
    /// `key` is the key whose object is being read.
    Map {
        parent: Option<(ObjId, u32)>,
        items: Vec<(Text, Slot)>,
        key: Option<Text>,
    },
    Named {
        parent: Option<(ObjId, u32)>,
        slot: Option<Slot>,
    },
}

/// A field read, into the object on top.
fn field(frame: &mut Frame, id: u32, value: Value) {
    if let Frame::Obj { fields, .. } = frame {
        fields.push(Field { id, value });
    }
}

/// An object slot read (an object finished, a reference or none), into what's on top.
fn deliver(stack: &mut [Frame], s: Slot) {
    match stack.last_mut() {
        Some(Frame::Obj { fields, pending, .. }) => fields.push(Field { id: *pending, value: Value::Obj(s) }),
        Some(Frame::Single { slot, .. } | Frame::Named { slot, .. }) => *slot = Some(s),
        Some(Frame::List { items, .. }) => items.push(s),
        // A map's key is always read before its object.
        Some(Frame::Map { items, key, .. }) => items.extend(key.take().map(|k| (k, s))),
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_stay_small() {
        // A big project holds millions of fields; the C# reader's memory once paused the app.
        assert!(size_of::<Value>() <= 32, "{}", size_of::<Value>());
        assert!(size_of::<Field>() <= 40, "{}", size_of::<Field>());
    }
}
