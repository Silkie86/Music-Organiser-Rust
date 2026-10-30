//! The field registry: everything the app knows about one DAW's file format, written down once. Format Lab names what
//! it shows with it, and the census counts what isn't in it. The data is in `registry/*.json`, copied from the C#
//! app's `Core/Formats/Registry`, which stays the source of truth until the Rust app takes it over.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Deserialize;

/// One thing the app knows about a DAW's file format: a Bitwig class or field, an FL Studio event, or a record inside
/// one, with what it means and how that was found out.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Entry {
    /// The id as the format writes it: hex for Bitwig ("0x9a"), decimal for FL Studio ("156"); a metadata key's name.
    pub id: String,
    pub name: String,
    pub about: String,
    /// Bitwig: the value's type. FL Studio events: byte, word, dword, text, ascii or data. Records inside them: u32,
    /// i32, fourcc (a VST2 ID), utf8 or bytes.
    #[serde(rename = "type")]
    pub kind: Option<String>,
    /// FL Studio data: named numbers at fixed offsets, e.g. "pan i32@0, volume i32@4".
    pub layout: Option<String>,
    /// FL Studio data made of equal records: their size, where the first starts, and their layout.
    pub record: Option<usize>,
    pub record_at: Option<usize>,
    pub record_layout: Option<String>,
    /// A record size that changed with FL's version: from version (major) to size.
    pub record_by_version: Option<HashMap<String, usize>>,
    /// The DAW versions it has been seen in.
    pub seen: Option<String>,
    /// How it was found out or checked: a file, a test, or a confirmation.
    pub proof: Option<String>,
    /// The constant that holds this id in the C# app's code, if one does.
    pub code: Option<String>,
}

impl Entry {
    /// The id as a number, or none when it isn't one (metadata keys).
    pub fn number(&self) -> Option<u32> {
        match self.id.get(..2) {
            Some("0x" | "0X") => u32::from_str_radix(self.id.get(2..)?, 16).ok(),
            _ => self.id.parse().ok(),
        }
    }

    /// The record size for an FL Studio version: the size from the highest version at or below `major`, else
    /// [`Entry::record`].
    pub fn record_size(&self, major: u32) -> Option<usize> {
        let Some(by_version) = &self.record_by_version else { return self.record };
        by_version
            .iter()
            .filter_map(|(v, &size)| Some((v.parse::<u32>().ok()?, size)))
            .filter(|&(v, _)| v <= major)
            .max_by_key(|&(v, _)| v)
            .map(|(_, size)| size)
            .or(self.record)
    }
}

/// One DAW's registry.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Registry {
    pub format: String,
    pub about: String,
    /// Bitwig: classes of object.
    pub classes: Vec<Entry>,
    /// Bitwig: fields of objects (one numbering for all classes).
    pub fields: Vec<Entry>,
    /// Bitwig: metadata keys, by name in [`Entry::id`].
    pub meta: Vec<Entry>,
    /// FL Studio: events.
    pub events: Vec<Entry>,
    /// FL Studio: records in a Fruity Wrapper state (event 213 of a plugin FL hosts).
    pub wrapper: Vec<Entry>,
    /// FL Studio: records in a VST3 plugin's state (the Fruity Wrapper's record 53).
    pub vst3: Vec<Entry>,
    #[serde(skip)]
    index: Index,
}

/// Where each entry is, by id, made once when the registry is read.
#[derive(Debug, Default)]
struct Index {
    classes: HashMap<u32, usize>,
    fields: HashMap<u32, usize>,
    events: HashMap<u32, usize>,
    wrapper: HashMap<u32, usize>,
    vst3: HashMap<u32, usize>,
    meta: HashMap<String, usize>,
}

impl Registry {
    /// Reads a registry from its JSON, checking that every id is written once and is a number where it should be.
    pub fn parse(json: &str) -> Result<Registry, String> {
        let mut r: Registry =
            serde_json::from_str(json).map_err(|e| format!("The field registry can't be read: {e}"))?;
        let by_number = |list: &[Entry], what: &str| -> Result<HashMap<u32, usize>, String> {
            let mut index = HashMap::new();
            for (i, e) in list.iter().enumerate() {
                let n = e.number().ok_or_else(|| format!("The {what} id \"{}\" isn't a number", e.id))?;
                if index.insert(n, i).is_some() {
                    return Err(format!("The {what} id {} is written twice", e.id));
                }
            }
            Ok(index)
        };
        r.index = Index {
            classes: by_number(&r.classes, "class")?,
            fields: by_number(&r.fields, "field")?,
            events: by_number(&r.events, "event")?,
            wrapper: by_number(&r.wrapper, "wrapper record")?,
            vst3: by_number(&r.vst3, "VST3 record")?,
            meta: r.meta.iter().enumerate().map(|(i, e)| (e.id.clone(), i)).collect(),
        };
        Ok(r)
    }

    pub fn class(&self, id: u32) -> Option<&Entry> {
        self.index.classes.get(&id).and_then(|&i| self.classes.get(i))
    }

    pub fn field(&self, id: u32) -> Option<&Entry> {
        self.index.fields.get(&id).and_then(|&i| self.fields.get(i))
    }

    pub fn event(&self, id: u32) -> Option<&Entry> {
        self.index.events.get(&id).and_then(|&i| self.events.get(i))
    }

    pub fn wrapper_record(&self, id: u32) -> Option<&Entry> {
        self.index.wrapper.get(&id).and_then(|&i| self.wrapper.get(i))
    }

    pub fn vst3_record(&self, id: u32) -> Option<&Entry> {
        self.index.vst3.get(&id).and_then(|&i| self.vst3.get(i))
    }

    pub fn meta_key(&self, key: &str) -> Option<&Entry> {
        self.index.meta.get(key).and_then(|&i| self.meta.get(i))
    }
}

/// Bitwig's registry, built into the tool.
pub fn bitwig() -> &'static Registry {
    static R: OnceLock<Registry> = OnceLock::new();
    R.get_or_init(|| built_in(include_str!("../registry/bitwig.json")))
}

/// FL Studio's registry, built into the tool.
pub fn fl_studio() -> &'static Registry {
    static R: OnceLock<Registry> = OnceLock::new();
    R.get_or_init(|| built_in(include_str!("../registry/fl-studio.json")))
}

/// A registry built into the tool. Its tests read both, so one that doesn't read is caught before it's built; should
/// one still not, Format Lab names nothing rather than stopping.
fn built_in(json: &str) -> Registry {
    Registry::parse(json).unwrap_or_default()
}

/// One named number in an FL Studio layout: "pan i32@0" is a 32-bit integer called pan at byte 0.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayoutField {
    pub name: String,
    pub kind: String,
    pub offset: usize,
}

/// A number a layout field read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Number {
    Int(i64),
    Float(f64),
}

impl LayoutField {
    /// How many bytes it takes: 1 to 8.
    pub fn size(&self) -> usize {
        size_of(&self.kind)
    }

    /// Reads "pan i32@0, volume i32@4": each a name, a type (u8 to u64, i8 to i64, f32, f64) and @offset.
    pub fn parse(layout: Option<&str>) -> Result<Vec<LayoutField>, String> {
        let mut fields = Vec::new();
        for part in layout.unwrap_or_default().split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let wrong = || format!("Can't read the layout \"{part}\": it should be a name, a type and @offset");
            let (name, spec) = part.rsplit_once(' ').ok_or_else(wrong)?;
            let (kind, offset) = spec.split_once('@').ok_or_else(wrong)?;
            if name.is_empty() || size_of(kind) == 0 || offset.is_empty() || !offset.bytes().all(|b| b.is_ascii_digit())
            {
                return Err(wrong());
            }
            let offset = offset.parse().map_err(|_| wrong())?;
            fields.push(LayoutField { name: name.to_owned(), kind: kind.to_owned(), offset });
        }
        Ok(fields)
    }

    /// The number at its offset in `data` (little-endian, as FL writes them), or none past the end.
    pub fn read(&self, data: &[u8]) -> Option<Number> {
        let b = data.get(self.offset..self.offset.checked_add(self.size())?)?;
        let n = |len| -> Option<[u8; 8]> {
            let mut a = [0; 8];
            a.get_mut(..len)?.copy_from_slice(b.get(..len)?);
            Some(a)
        };
        let signed = |len: usize| -> Option<i64> {
            let shift = 64 - 8 * len as u32;
            Some((i64::from_le_bytes(n(len)?) << shift) >> shift)
        };
        Some(match self.kind.as_str() {
            "u8" | "u16" | "u32" => Number::Int(i64::from_le_bytes(n(self.size())?)),
            "u64" => Number::Int(u64::from_le_bytes(n(8)?) as i64),
            "i8" | "i16" | "i32" | "i64" => Number::Int(signed(self.size())?),
            "f32" => Number::Float(f64::from(f32::from_le_bytes(b.first_chunk::<4>().copied()?))),
            "f64" => Number::Float(f64::from_le_bytes(n(8)?)),
            _ => return None,
        })
    }
}

fn size_of(kind: &str) -> usize {
    match kind {
        "u8" | "i8" => 1,
        "u16" | "i16" => 2,
        "u32" | "i32" | "f32" => 4,
        "u64" | "i64" | "f64" => 8,
        _ => 0,
    }
}
