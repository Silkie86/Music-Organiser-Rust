//! A Bitwig or FL Studio file laid out for Format Lab: everything in it, in file order, and how values are written.

use std::fmt::Write as _;
use std::path::Path;

use crate::{bitwig_lab, fl_lab};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Bitwig,
    FlStudio,
}

/// What an item holds, so it can be compared and searched as well as shown.
#[derive(Clone, Debug, PartialEq)]
pub enum Data {
    /// Only other things (an object, a list, an event made of parts), or nothing (a null).
    None,
    Text(String),
    Int(i64),
    Float(f64),
    Bytes(Vec<u8>),
    Ints(Vec<i64>),
    Floats(Vec<f64>),
    Texts(Vec<String>),
}

/// One thing Format Lab found in a file: an object or field (Bitwig), an event or part of one (FL Studio), a metadata
/// entry, or a file in a Bitwig project's zip.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    /// Names the same thing in two saves of a project, so they can be compared. Parts are separated by "/", and a
    /// thing's parts start with its own key.
    pub key: String,
    /// Where it is, with the registry's names where it has them.
    pub path: String,
    /// How deep it sits, for showing the file as a tree.
    pub depth: usize,
    /// Its own part of the path.
    pub label: String,
    /// What it holds, as text.
    pub value: String,
    pub data: Data,
    /// It holds other things rather than a value.
    pub is_group: bool,
    /// Where its bytes sit in the data of the item that holds it, when that's known (FL Studio's parts), and how many
    /// bytes it takes there.
    pub span: Option<(usize, usize)>,
}

impl Item {
    pub fn new(key: String, path: String, depth: usize, label: String, value: String, data: Data) -> Item {
        Item { key, path, depth, label, value, data, is_group: false, span: None }
    }

    pub fn group(key: String, path: String, depth: usize, label: String, value: String) -> Item {
        Item { is_group: true, ..Item::new(key, path, depth, label, value, Data::None) }
    }
}

/// A Bitwig or FL Studio file laid out for Format Lab.
#[derive(Clone, Debug)]
pub struct LabFile {
    pub name: String,
    pub format: Format,
    /// The DAW's version as the file gives it, or "".
    pub version: String,
    pub items: Vec<Item>,
    /// Why part of the file couldn't be read, or none when all of it was.
    pub problem: Option<String>,
    /// What the reader said about a file it could still read, but where something was off (an FL save cut short, or
    /// with a garbled header).
    pub warning: Option<String>,
}

impl LabFile {
    pub fn daw(&self) -> &'static str {
        match self.format {
            Format::Bitwig => "Bitwig Studio",
            Format::FlStudio => "FL Studio",
        }
    }

    pub fn open(path: &Path) -> Result<LabFile, String> {
        let raw = std::fs::read(path).map_err(|e| format!("Can't read {}: {e}", path.display()))?;
        let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
        LabFile::read(&raw, &name)
    }

    /// Lays out a Bitwig file (.bwproject, .bwpreset, .bwclip, .bwcurve) or an FL Studio project.
    pub fn read(raw: &[u8], name: &str) -> Result<LabFile, String> {
        if mo_formats::bitwig::is_bitwig(raw) {
            return bitwig_lab::read(raw, name);
        }
        if raw.starts_with(b"FLhd") {
            return fl_lab::read(raw, name);
        }
        Err(format!("{name} isn't a Bitwig or FL Studio file"))
    }
}

/// A number as .NET's round-trip format writes it, which the C# app's Format Lab used: the shortest text that reads
/// back as the same number, with an exponent from 1E+15 up and below 0.0001 ("1E-05").
pub fn float(d: f64) -> String {
    if d.is_infinite() {
        return if d > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    if d.is_nan() || d == 0.0 {
        return format!("{d}");
    }
    // Rust's shortest digits, as "1.2345e-5".
    let sci = format!("{d:e}");
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    if (-4..15).contains(&exp) {
        format!("{d}")
    } else {
        format!("{mantissa}E{}{:02}", if exp < 0 { '-' } else { '+' }, exp.abs())
    }
}

/// A count with its thousands marked: 1,234.
pub fn count(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Text in quotes, with what can't be shown escaped, and cut to 300 letters.
pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    let mut letters = s.chars();
    for c in letters.by_ref().take(300) {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    if letters.next().is_some() {
        out.push('…');
    }
    out.push('"');
    out
}

/// Bytes as "<N bytes>" with a look at what they start with: text when they read as text, else hex.
pub fn bytes(b: &[u8]) -> String {
    let size = if b.len() == 1 { "<1 byte>".to_owned() } else { format!("<{} bytes>", count(b.len())) };
    if b.is_empty() {
        return size;
    }
    if let Some(text) = as_text(b) {
        let shown = match text.char_indices().nth(60) {
            Some((at, _)) => format!("{}…", text.get(..at).unwrap_or_default()),
            None => text,
        };
        return format!("{size} {}", quote(&shown));
    }
    let more = if b.len() > 16 { " …" } else { "" };
    format!("{size} {}{more}", hex(b.get(..16).unwrap_or(b)))
}

pub fn hex(b: &[u8]) -> String {
    let mut out = String::with_capacity(b.len() * 3);
    for (i, x) in b.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let _ = write!(out, "{x:02x}");
    }
    out
}

/// The text bytes hold, when all of them are printable UTF-16 (little-endian) or UTF-8; else none.
pub fn as_text(b: &[u8]) -> Option<String> {
    let trimmed = b.iter().rposition(|&x| x != 0).map_or(&b[..0], |end| b.get(..=end).unwrap_or(b));
    if trimmed.len() < 2 {
        return None;
    }
    // UTF-16: every other byte zero for Latin text, an even length with its terminator.
    if b.len().is_multiple_of(2) && b.len() >= 4 && b.get(1) == Some(&0) {
        let units: Vec<u16> = b.as_chunks::<2>().0.iter().map(|&c| u16::from_le_bytes(c)).collect();
        let s = String::from_utf16_lossy(&units);
        let s = s.trim_end_matches('\0');
        if !s.is_empty() && s.chars().all(printable) {
            return Some(s.to_owned());
        }
    }
    let s = std::str::from_utf8(trimmed).ok()?;
    s.chars().all(printable).then(|| s.to_owned())
}

fn printable(c: char) -> bool {
    !c.is_control() || matches!(c, '\n' | '\r' | '\t')
}
