//! The format registry test (spec section 11.2), for the mo-formats readers: every FL Studio and Bitwig id the readers
//! in `crates/mo-formats/src/fl` and `src/bitwig` use is written down in `registry/*.json` with a name, and every id the
//! registry says a reader relies on is used by one (or is on a list of ones we know aren't used yet).
//!
//! Not covered yet: mo-convert's Bitwig writer (`crates/mo-convert/src/bitwig.rs`) also uses ids, written as bare
//! numbers; bringing it in is a follow-up.
//!
//! How it works: the test reads the readers' own source files (and any in folders under them) as plain text and picks
//! out the ids in them, so a new id in a reader is checked without anyone having to remember to add it here.
//!
//! - Most ids are named constants (`const EV_TEMPO: u8 = 156;`, `const F_START: u32 = 0x2af;`), and the name says what
//!   kind of id it is: `EV_` an FL event, `VST_` a Fruity Wrapper record, `CLS_` a Bitwig class, `F_` a Bitwig field.
//!   Every other `u8` or `u32` constant must be in OTHER_NAMED (an id named some other way) or NOT_IDS.
//! - In the Bitwig files that hold ids, every hex number outside a constant must be in BARE (an id written as a bare
//!   number) or NOT_ID_HEX (a hex number that isn't an id). This "fails closed": an id passed through a variable, or
//!   on a line rustfmt wrapped, is still found, because nothing in those files is skipped.
//! - FL's bare ids are the VST3 state's records, read as `records.get(&n)`.
//!
//! Every table below is checked both ways: an entry the scan no longer finds fails the test, so the tables can't go
//! stale without a word.
//!
//! Only the code before a file's `#[cfg(test)] mod tests` counts: ids made up in a reader's own tests aren't the
//! reader's. The registry and the sources are read from this repository (CARGO_MANIFEST_DIR), never a user folder.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use mo_formatlab::registry::{Entry, Registry};

/// Which list of the registry an id belongs in. FL Studio and Bitwig number their things separately, and within Bitwig
/// classes and fields are numbered separately too, so an id only means something together with its list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum List {
    /// FL Studio: an event in the project's event stream ("events").
    FlEvent,
    /// FL Studio: a record in the Fruity Wrapper's state ("wrapper").
    FlWrapper,
    /// FL Studio: a record in a VST3 plugin's state ("vst3").
    FlVst3,
    /// Bitwig: a class of object ("classes").
    BwClass,
    /// Bitwig: a field of an object ("fields").
    BwField,
}

use List::*;

/// The Bitwig files that hold ids. Every hex number in them (outside a constant) is checked.
const BITWIG_ID_FILES: &[&str] =
    &["bitwig/reader.rs", "bitwig/arrangement.rs", "bitwig/automation.rs", "bitwig/curve.rs"];

/// The Bitwig files that are the codec (how any object is read and written, whatever its class): their hex numbers
/// are value types and byte patterns, not ids, so only their constants are checked. A new Bitwig file must go in one
/// of these two lists.
const BITWIG_CODEC_FILES: &[&str] = &["bitwig/format.rs", "bitwig/encode.rs", "bitwig/mod.rs"];

/// Constants in the readers whose names don't start with `EV_`, `VST_`, `CLS_` or `F_` but which are ids all the same:
/// (file, constant, which list).
const OTHER_NAMED: &[(&str, &str, List)] = &[
    ("bitwig/automation.rs", "CLIP", BwClass),
    ("bitwig/automation.rs", "LANE", BwClass),
    ("bitwig/curve.rs", "CURVE", BwClass),
    ("bitwig/curve.rs", "POINT", BwClass),
    ("bitwig/curve.rs", "POINTS", BwField),
    ("bitwig/curve.rs", "Y", BwField),
    ("bitwig/curve.rs", "X", BwField),
    ("bitwig/curve.rs", "BEND", BwField),
    ("bitwig/curve.rs", "NAME", BwField),
    ("bitwig/curve.rs", "CREATOR", BwField),
    ("bitwig/curve.rs", "COMMENT", BwField),
    ("bitwig/curve.rs", "CATEGORY", BwField),
    ("bitwig/curve.rs", "TAGS", BwField),
    ("bitwig/encode.rs", "DEVICE_TYPE", BwField),
];

/// Number constants in the reader files that aren't ids of the file format: (file, constant).
const NOT_IDS: &[(&str, &str)] = &[
    // The readers' own version numbers (rule 7), not the file's.
    ("fl/mod.rs", "VERSION"),
    ("bitwig/reader.rs", "VERSION"),
    // FL: colours FL gives a track or channel that doesn't say.
    ("fl/arrangement.rs", "DEFAULT_TRACK_COLOUR"),
    ("fl/arrangement.rs", "DEFAULT_CHANNEL_COLOUR"),
    // FL: values inside an automation link (event 227), not events.
    ("fl/automation.rs", "TARGET_VOLUME"),
    ("fl/automation.rs", "TARGET_PAN"),
    ("fl/automation.rs", "TARGET_PITCH"),
    ("fl/automation.rs", "TARGET_ON_OFF"),
    ("fl/automation.rs", "TARGET_PLUGIN"),
    ("fl/automation.rs", "TARGET_SLOT_ON_OFF"),
    ("fl/automation.rs", "TARGET_INSERT_VOLUME"),
    ("fl/automation.rs", "TARGET_INSERT_PAN"),
    ("fl/automation.rs", "TARGET_SONG"),
    ("fl/automation.rs", "TARGET_MAIN_VOLUME"),
    ("fl/automation.rs", "TARGET_TEMPO"),
    ("fl/automation.rs", "FL8_PLUGIN"),
    // FL: values of a channel's type (event 21) and of the wrapper's kind (record 50).
    ("fl/mod.rs", "CHANNEL_TYPE_INSTRUMENT"),
    ("fl/song.rs", "CHANNEL_TYPE_AUTOMATION"),
    ("fl/mod.rs", "KIND_VST3"),
    // FL: the wrapper version FL 7 and 8 save, and the playlist's pattern numbering (item above 20480).
    ("fl/mod.rs", "OLD_WRAPPER"),
    ("fl/song.rs", "PATTERN_BASE"),
];

/// Ids the readers write as bare numbers rather than named constants: (file, which list, id). For Bitwig the list is
/// checked against how the number is used: making an object (`Object::new(..)`) or comparing an object's `.class`
/// means a class, anything else a field.
const BARE: &[(&str, List, u32)] = &[
    // An automation clip's points, name, place and what its lane moves.
    ("bitwig/automation.rs", BwField, 0x2ae9),
    ("bitwig/automation.rs", BwField, 0x10e9),
    ("bitwig/automation.rs", BwField, 0x48e),
    ("bitwig/automation.rs", BwField, 0x21f),
    ("bitwig/automation.rs", BwField, 0x2af),
    ("bitwig/automation.rs", BwField, 0x28f),
    ("bitwig/automation.rs", BwField, 0xba6),
    ("bitwig/automation.rs", BwField, 0x2a3),
    ("bitwig/automation.rs", BwField, 0x133b),
    ("bitwig/automation.rs", BwField, 0x133a),
    ("bitwig/automation.rs", BwField, 0x2d24),
    ("bitwig/automation.rs", BwField, 0x26),
    ("bitwig/automation.rs", BwField, 0x10f8),
    // A curve file's fields whose meaning isn't known; the curve writer saves them as Bitwig does.
    ("bitwig/curve.rs", BwField, 0x362d),
    ("bitwig/curve.rs", BwField, 0x3646),
    ("bitwig/curve.rs", BwField, 0x3601),
    ("bitwig/curve.rs", BwField, 0x3602),
    ("bitwig/curve.rs", BwField, 0x3635),
    ("bitwig/curve.rs", BwField, 0x3788),
    ("bitwig/curve.rs", BwField, 0x3665),
    ("bitwig/curve.rs", BwField, 0x3666),
    // A VST3's state in FL: 2 the controller's state, 3 the component's, 4 the setting ids.
    ("fl/presets.rs", FlVst3, 2),
    ("fl/presets.rs", FlVst3, 3),
    ("fl/presets.rs", FlVst3, 4),
];

/// Hex numbers in the Bitwig id files that aren't ids: (file, number, what it is).
const NOT_ID_HEX: &[(&str, u32, &str)] = &[
    ("bitwig/reader.rs", 0xff, "the low byte of a UTF-16 text's length"),
    ("bitwig/reader.rs", 0x80, "a text's header byte, and the end of plain ASCII"),
    ("bitwig/reader.rs", 0x78, "the first byte of a zlib stream"),
    ("bitwig/reader.rs", 0x7800, "a zlib stream's first byte, for its check"),
    ("bitwig/curve.rs", 0xff, "the value the curve writer puts in three unknown fields"),
];

/// Bitwig metadata keys the readers use (by name, not number): (file, key). The test finds keys read with `meta(..)`,
/// `meta_str(..)` or `meta_strings(..)`, and keys written as `("key", value)` pairs (the curve writer's).
const META: &[(&str, &str)] = &[
    ("bitwig/reader.rs", "application_version_name"),
    ("bitwig/reader.rs", "title"),
    ("bitwig/reader.rs", "used_files"),
    ("bitwig/reader.rs", "structure"),
    ("bitwig/curve.rs", "application_version_name"),
    ("bitwig/curve.rs", "beat_length"),
    ("bitwig/curve.rs", "branch"),
    ("bitwig/curve.rs", "comment"),
    ("bitwig/curve.rs", "creator"),
    ("bitwig/curve.rs", "curve_category"),
    ("bitwig/curve.rs", "curve_kind"),
    ("bitwig/curve.rs", "revision_id"),
    ("bitwig/curve.rs", "revision_no"),
    ("bitwig/curve.rs", "tags"),
    ("bitwig/curve.rs", "type"),
];

/// KNOWN GAPS, for Solomon to judge: ids a reader uses that the registry doesn't list yet. Their names aren't
/// guessed here; once one is added to `registry/*.json` the test says to take it off this list.
///
/// The eight are the unknown fields of a Bitwig curve file (class 0x127f). The registry's entry for the class mentions
/// them in its "about" ("Unknown: 0x362d u8 0, 0x3646 bool 1, ..."), but none has an entry of its own in "fields".
const KNOWN_GAPS: &[(List, u32)] = &[
    (BwField, 0x362d),
    (BwField, 0x3646),
    (BwField, 0x3601),
    (BwField, 0x3602),
    (BwField, 0x3635),
    (BwField, 0x3788),
    (BwField, 0x3665),
    (BwField, 0x3666),
];

/// Ids the registry gives a C# constant name ("code", so the C# readers rely on them) that no Rust reader uses yet.
/// Each is fine for now; the test only makes sure the list stays true.
const KNOWINGLY_UNUSED: &[(List, u32)] = &[
    // ClsPluginTarget: what an automation lane moves when it's a plugin's setting. The Rust readers take the lane's
    // names (0x133b, 0x133a) instead.
    (BwClass, 0x65),
    // ClsVst2Slot and ClsVst3Slot: a plugin in a track's device chain. The Rust reader finds plugins by their own
    // classes (0x1a5, 0x728).
    (BwClass, 0x1a6),
    (BwClass, 0x729),
];

// ---- Reading the registry and the readers' sources ----

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn registry(file: &str) -> Registry {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("registry").join(file);
    let json = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    Registry::parse(&json).unwrap()
}

/// Both registries, so one lookup finds an id in the right list.
struct Registries {
    fl: Registry,
    bitwig: Registry,
}

impl Registries {
    fn read() -> Registries {
        Registries { fl: registry("fl-studio.json"), bitwig: registry("bitwig.json") }
    }

    fn entry(&self, list: List, id: u32) -> Option<&Entry> {
        match list {
            FlEvent => self.fl.event(id),
            FlWrapper => self.fl.wrapper_record(id),
            FlVst3 => self.fl.vst3_record(id),
            BwClass => self.bitwig.class(id),
            BwField => self.bitwig.field(id),
        }
    }

    /// Every entry in a list, with its id as a number.
    fn all(&self, list: List) -> Vec<(u32, &Entry)> {
        let entries = match list {
            FlEvent => &self.fl.events,
            FlWrapper => &self.fl.wrapper,
            FlVst3 => &self.fl.vst3,
            BwClass => &self.bitwig.classes,
            BwField => &self.bitwig.fields,
        };
        entries.iter().map(|e| (e.number().unwrap(), e)).collect()
    }
}

/// The `.rs` files in a folder and every folder under it.
fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// The code before the file's tests: everything up to a `#[cfg(test)]` followed by `mod ..`. A `#[cfg(test)]` on
/// anything else is an error, so test code can't be cut off (or let in) by mistake.
fn without_tests(text: &str, file: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        if line.trim() == "#[cfg(test)]" {
            let next = lines[i + 1..].iter().map(|l| l.trim()).find(|l| !l.is_empty()).unwrap_or_default();
            assert!(
                next.starts_with("mod ") || next.starts_with("pub mod "),
                "{file}: a #[cfg(test)] that isn't on a test module (\"{next}\"); teach this test what it covers"
            );
            return lines[..i].join("\n");
        }
    }
    text.to_owned()
}

/// The reader files: (path from `crates/mo-formats/src`, the code before its tests).
fn sources() -> Vec<(String, String)> {
    let src = repo().join("crates/mo-formats/src");
    let mut paths = Vec::new();
    for folder in ["fl", "bitwig"] {
        rust_files(&src.join(folder), &mut paths);
    }
    let mut files: Vec<(String, String)> = paths
        .iter()
        .map(|path| {
            let file = path.strip_prefix(&src).unwrap().to_string_lossy().replace('\\', "/");
            let code = without_tests(&std::fs::read_to_string(path).unwrap(), &file);
            (file, code)
        })
        .collect();
    files.sort();
    assert!(files.len() > 5, "the reader files weren't found under {}", src.display());
    files
}

/// The code's lines, leaving out comments (a `//` comment may mention ids the line doesn't use).
fn code_lines(code: &str) -> impl Iterator<Item = &str> {
    code.lines().map(|l| l.split("//").next().unwrap_or_default())
}

/// A number as Rust writes it: hex after "0x" (either case) or decimal, with any `_` between the digits.
fn number(text: &str) -> Option<u32> {
    let text = text.replace('_', "");
    match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => u32::from_str_radix(hex, 16).ok(),
        None => text.parse().ok(),
    }
}

/// Whether the name says it's an id: `EV_` or `VST_` in an FL file, `CLS_` or `F_` in a Bitwig one.
fn id_list_by_prefix(file: &str, name: &str) -> Option<List> {
    let fl = file.starts_with("fl/");
    match () {
        _ if fl && name.starts_with("EV_") => Some(FlEvent),
        _ if fl && name.starts_with("VST_") => Some(FlWrapper),
        _ if !fl && name.starts_with("CLS_") => Some(BwClass),
        _ if !fl && name.starts_with("F_") => Some(BwField),
        _ => None,
    }
}

/// A constant written on one line: its name, type and value as text. None for a line that isn't one, or a `const fn`
/// or a generic's `const N: usize`.
fn constant(line: &str) -> Option<(&str, &str, &str)> {
    let rest = line.trim().strip_prefix("pub ").unwrap_or(line.trim());
    let rest = ["pub(crate) ", "pub(super) "].iter().fold(rest, |r, p| r.strip_prefix(p).unwrap_or(r));
    let rest = rest.strip_prefix("const ")?;
    let (name, rest) = rest.split_once(':')?;
    let name = name.trim();
    if !name.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_') {
        return None;
    }
    let (kind, value) = rest.split_once('=').unwrap_or((rest, ""));
    Some((name, kind.trim(), value.trim().trim_end_matches(';').trim()))
}

/// Every `u8` or `u32` constant in the code: (name, value). Problems for an id constant (by its name or OTHER_NAMED)
/// whose value can't be read: another type, a suffix, or a value on the next line. One set from another constant
/// (`F_ITEMS = F_NOTES`) is fine and left out; the one it copies is found where it's set.
fn number_constants(file: &str, code: &str, problems: &mut Vec<String>) -> Vec<(String, u32)> {
    let mut found = Vec::new();
    for line in code_lines(code) {
        let Some((name, kind, value)) = constant(line) else { continue };
        let is_id =
            id_list_by_prefix(file, name).is_some() || OTHER_NAMED.iter().any(|(f, n, _)| *f == file && *n == name);
        let copy = !value.is_empty() && value.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
        match (matches!(kind, "u8" | "u32"), number(value)) {
            (true, Some(n)) => found.push((name.to_owned(), n)),
            _ if !is_id || copy => {}
            _ => problems.push(format!("{file}: const {name}: {kind} = \"{value}\" can't be read as a u8 or u32 id")),
        }
    }
    found
}

/// A hex number found in Bitwig code, and whether where it's written says it's a class.
#[derive(Debug, PartialEq)]
struct Hex {
    n: u32,
    class: bool,
}

/// Every hex number in the code outside constants. A number counts as a class when it makes an object
/// (`Object::new(0x..)`) or is compared with an object's class (`.class == 0x..`, `0x.. == o.class`,
/// `matches!(o.class, 0x.. | ..)`); anything else is taken as a field. A number too big for a u32 is a problem. A call
/// rustfmt wraps (`Object::new(` on one line, its number on the next) still has its number found, but taken as a
/// field, so BARE's class-or-field check then points at it.
fn hex_numbers(file: &str, code: &str, problems: &mut Vec<String>) -> Vec<Hex> {
    let mut found = Vec::new();
    for line in code_lines(code).filter(|l| constant(l).is_none()) {
        let lower = line.to_ascii_lowercase();
        for (at, _) in lower.match_indices("0x") {
            if lower[..at].ends_with(|c: char| c.is_ascii_alphanumeric() || c == '_') {
                continue;
            }
            let digits: String = lower[at + 2..].chars().take_while(|c| c.is_ascii_hexdigit() || *c == '_').collect();
            let Some(n) = number(&format!("0x{digits}")) else {
                problems.push(format!("{file}: can't read the number at \"{}\"", line.trim()));
                continue;
            };
            let (before, after) = (lower[..at].trim_end(), lower[at + 2 + digits.len()..].trim_start());
            let class = before.ends_with("object::new(")
                || before.ends_with(".class ==")
                || before.ends_with(".class !=")
                || ((after.starts_with("==") || after.starts_with("!=")) && after.contains(".class"))
                || before.rfind("matches!(").is_some_and(|m| before[m..].contains(".class"));
            found.push(Hex { n, class });
        }
    }
    found
}

/// Every FL VST3 record read as `records.get(&n)` with a number n (the VST3 state's records are kept in `records`;
/// other maps read with `get(&..)` aren't counted).
fn bare_fl_records(code: &str) -> BTreeSet<u32> {
    let mut found = BTreeSet::new();
    for line in code_lines(code) {
        for (at, s) in line.match_indices("records.get(&") {
            let digits: String = line[at + s.len()..].chars().take_while(char::is_ascii_digit).collect();
            if let Some(n) = number(&digits) {
                found.insert(n);
            }
        }
    }
    found
}

/// Every metadata key read with `meta("..")`, `meta_str("..")` or `meta_strings("..")`, or written as a
/// `("key", value)` pair.
fn meta_keys(code: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for call in ["meta(\"", "meta_str(\"", "meta_strings(\"", "(\""] {
        for (at, _) in code.match_indices(call) {
            let rest = &code[at + call.len()..];
            let Some(end) = rest.find('"') else { continue };
            let key = &rest[..end];
            let pair = call == "(\"";
            if pair && !rest[end + 1..].starts_with(',') {
                continue;
            }
            if !key.is_empty() && key.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
                found.insert(key.to_owned());
            }
        }
    }
    found
}

/// `EV_PATTERN_NOTES` as the C# app writes it: `EvPatternNotes`.
fn pascal_case(name: &str) -> String {
    name.split('_')
        .map(|w| {
            let mut chars = w.chars();
            chars
                .next()
                .map(|c| c.to_ascii_uppercase().to_string() + &chars.as_str().to_lowercase())
                .unwrap_or_default()
        })
        .collect()
}

/// One id a reader uses, and where.
#[derive(Debug)]
struct Use {
    file: String,
    /// The constant that holds it, or none for a bare number.
    constant: Option<String>,
    list: List,
    id: u32,
}

/// Every id the readers use, found in their sources. Fails when something in the sources isn't accounted for by the
/// tables above, or a table has an entry the sources no longer have.
fn ids_in_use() -> Vec<Use> {
    let mut uses = Vec::new();
    let mut problems = Vec::new();
    let mut other_named_seen = BTreeSet::new();
    let mut not_ids_seen = BTreeSet::new();
    let mut bare_seen = BTreeSet::new();
    let mut not_id_hex_seen = BTreeSet::new();

    for (file, code) in sources() {
        // Named constants.
        for (name, id) in number_constants(&file, &code, &mut problems) {
            let other = OTHER_NAMED.iter().position(|(f, n, _)| *f == file && *n == name);
            let list = id_list_by_prefix(&file, &name).or_else(|| other.map(|i| OTHER_NAMED[i].2));
            other_named_seen.extend(other);
            match list {
                Some(list) => uses.push(Use { file: file.clone(), constant: Some(name), list, id }),
                None => match NOT_IDS.iter().position(|(f, n)| *f == file && *n == name) {
                    Some(i) => {
                        not_ids_seen.insert(i);
                    }
                    None => problems.push(format!(
                        "{file}: const {name} = {id} isn't known to this test: add it to OTHER_NAMED (an id) or NOT_IDS"
                    )),
                },
            }
        }

        // Bare numbers.
        if file.starts_with("bitwig/") {
            let id_file = BITWIG_ID_FILES.contains(&file.as_str());
            if !id_file && !BITWIG_CODEC_FILES.contains(&file.as_str()) {
                problems.push(format!("{file} is new: add it to BITWIG_ID_FILES or BITWIG_CODEC_FILES"));
            }
            if !id_file {
                continue;
            }
            for hex in hex_numbers(&file, &code, &mut problems) {
                if let Some(i) = NOT_ID_HEX.iter().position(|&(f, n, _)| f == file && n == hex.n) {
                    not_id_hex_seen.insert(i);
                    continue;
                }
                let Some(i) = BARE.iter().position(|&(f, _, n)| f == file && n == hex.n) else {
                    problems.push(format!(
                        "{file}: the number {:#x} isn't in this test's BARE (an id) or NOT_ID_HEX (not an id)",
                        hex.n
                    ));
                    continue;
                };
                let (_, list, id) = BARE[i];
                let found_as = if hex.class { BwClass } else { BwField };
                if list != found_as {
                    problems.push(format!("{file}: {id:#x} is used as a {found_as:?} there, but BARE says {list:?}"));
                }
                if bare_seen.insert(i) {
                    uses.push(Use { file: file.clone(), constant: None, list, id });
                }
            }
        } else {
            for n in bare_fl_records(&code) {
                match BARE.iter().position(|&(f, list, id)| f == file && list == FlVst3 && id == n) {
                    Some(i) => {
                        bare_seen.insert(i);
                        uses.push(Use { file: file.clone(), constant: None, list: FlVst3, id: n });
                    }
                    None => problems.push(format!("{file}: the VST3 record {n} isn't in this test's BARE table")),
                }
            }
        }
    }

    // Table entries the sources no longer have.
    for (i, (file, name, _)) in OTHER_NAMED.iter().enumerate().filter(|(i, _)| !other_named_seen.contains(i)) {
        problems.push(format!("{file} no longer has const {name}: take it off OTHER_NAMED ({i})"));
    }
    for (file, name) in NOT_IDS.iter().enumerate().filter(|(i, _)| !not_ids_seen.contains(i)).map(|(_, e)| e) {
        problems.push(format!("{file} no longer has const {name}: take it off NOT_IDS"));
    }
    for (file, _, id) in BARE.iter().enumerate().filter(|(i, _)| !bare_seen.contains(i)).map(|(_, e)| e) {
        problems.push(format!("{file} no longer has the id {id:#x} ({id}): take it off BARE"));
    }
    for (file, n, _) in NOT_ID_HEX.iter().enumerate().filter(|(i, _)| !not_id_hex_seen.contains(i)).map(|(_, e)| e) {
        problems.push(format!("{file} no longer has the number {n:#x}: take it off NOT_ID_HEX"));
    }
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
    uses
}

// ---- The tests ----

/// Every id a reader uses is in the registry with a name, except the known gaps; and where the registry gives the C#
/// constant's name, the Rust constant has the same name (so a constant can't point at the wrong id unnoticed).
#[test]
fn every_id_the_readers_use_is_in_the_registry_with_its_name() {
    let reg = Registries::read();
    let mut problems = Vec::new();
    for u in ids_in_use() {
        let at = match &u.constant {
            Some(name) => format!("{}: {name} ({:?} {:#x} = {})", u.file, u.list, u.id, u.id),
            None => format!("{}: bare {:?} {:#x} ({})", u.file, u.list, u.id, u.id),
        };
        let known_gap = KNOWN_GAPS.contains(&(u.list, u.id));
        match reg.entry(u.list, u.id) {
            None if known_gap => {}
            None => problems.push(format!("{at} isn't in the registry")),
            Some(_) if known_gap => problems.push(format!("{at} is in the registry now: take it off KNOWN_GAPS")),
            Some(e) if e.name.trim().is_empty() => problems.push(format!("{at} has no name in the registry")),
            Some(e) => {
                if let (Some(name), Some(code)) = (&u.constant, &e.code)
                    && pascal_case(name) != *code
                {
                    problems.push(format!("{at}: the registry's C# name is {code}, so it should be called that"));
                }
            }
        }
    }
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
}

/// Every id the registry gives a C# constant name (so a C# reader relies on it) is used by a Rust reader, or is on
/// KNOWINGLY_UNUSED. Ids the registry only writes down (seen in files, no reader needs them) aren't asked for.
#[test]
fn every_id_the_registry_says_a_reader_relies_on_is_used() {
    let reg = Registries::read();
    let used: BTreeSet<(List, u32)> = ids_in_use().iter().map(|u| (u.list, u.id)).collect();
    let mut problems = Vec::new();
    for list in [FlEvent, FlWrapper, FlVst3, BwClass, BwField] {
        for (id, e) in reg.all(list) {
            let Some(code) = &e.code else { continue };
            let in_use = used.contains(&(list, id));
            let unused_ok = KNOWINGLY_UNUSED.contains(&(list, id));
            if !in_use && !unused_ok {
                problems.push(format!("{list:?} {} ({}, C# {code}) isn't used by a Rust reader", e.id, e.name));
            }
            if in_use && unused_ok {
                problems.push(format!("{list:?} {} ({}) is used now: take it off KNOWINGLY_UNUSED", e.id, e.name));
            }
        }
    }
    for &(list, id) in KNOWINGLY_UNUSED {
        if reg.entry(list, id).is_none_or(|e| e.code.is_none()) {
            problems.push(format!("KNOWINGLY_UNUSED has {list:?} {id:#x}, which the registry doesn't name in C#"));
        }
    }
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
}

/// Every Bitwig metadata key the readers use is in the registry with a name, and the META table is exactly what the
/// sources have.
#[test]
fn every_metadata_key_the_readers_use_is_in_the_registry() {
    let reg = Registries::read();
    let mut problems = Vec::new();
    let mut found = BTreeSet::new();
    for (file, code) in sources().into_iter().filter(|(f, _)| f.starts_with("bitwig/")) {
        for key in meta_keys(&code) {
            if !META.contains(&(file.as_str(), key.as_str())) {
                problems.push(format!("{file}: the metadata key \"{key}\" isn't in this test's META table"));
            }
            found.insert((file.clone(), key));
        }
    }
    for &(file, key) in META {
        if !found.contains(&(file.to_owned(), key.to_owned())) {
            problems.push(format!("{file} no longer uses the metadata key \"{key}\": take it off META"));
        }
        match reg.bitwig.meta_key(key) {
            None => problems.push(format!("{file}: the metadata key \"{key}\" isn't in the registry")),
            Some(e) if e.name.trim().is_empty() => problems.push(format!("\"{key}\" has no name in the registry")),
            Some(_) => {}
        }
    }
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
}

/// The test's own helpers find what they should, so a passing test means something.
#[test]
fn the_source_scan_finds_constants_and_bare_ids() {
    let mut problems = Vec::new();
    let code = "pub(super) const F_START: u32 = 0x2af;\nconst EV_TEMPO: u8 = 156;\nconst F_ITEMS: u32 = F_NOTES;\n\
                const F_WIDE: u64 = 0x10;\nconst NAMES: &str = \"0x77\";\n";
    assert_eq!(
        number_constants("bitwig/x.rs", code, &mut problems),
        [("F_START".to_owned(), 0x2af), ("EV_TEMPO".to_owned(), 156)]
    );
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains("F_WIDE"));

    let mut problems = Vec::new();
    let code = "let x = field_obj(doc, o, 0x2AE9); // 0x9999 in a comment\nif o.class != 0xd90 {}\n\
                matches!(c.class, 0x47 | 0xEE)\nlet a = 0x1_0F8 == o.class;\nfor id in [0x3788] { root.set(id, \
                Value::U8(0xff)) }\nlet m = Object::new(\n0x108);\n";
    let found: Vec<(u32, bool)> =
        hex_numbers("bitwig/x.rs", code, &mut problems).iter().map(|h| (h.n, h.class)).collect();
    assert_eq!(
        found,
        [
            (0x2ae9, false),
            (0xd90, true),
            (0x47, true),
            (0xee, true),
            (0x10f8, true),
            (0x3788, false),
            (0xff, false),
            (0x108, false)
        ]
    );
    assert!(problems.is_empty());

    assert_eq!(bare_fl_records("records.get(&3)\nother.get(&5)"), BTreeSet::from([3]));
    assert_eq!(
        meta_keys("d.meta_str(\"title\") + d.meta(\"structure\")\n(\"branch\", text(\"releases\")),\n(\"\", 1)"),
        BTreeSet::from(["branch".into(), "structure".into(), "title".into()])
    );
    assert_eq!(without_tests("a\n#[cfg(test)]\n\nmod tests {\n0x1\n}", "x.rs"), "a");
    assert_eq!(pascal_case("EV_PATTERN_NOTES"), "EvPatternNotes");
    assert_eq!(pascal_case("CLS_HARDWARE_IO"), "ClsHardwareIo");
}
