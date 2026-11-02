//! The format registry test (spec section 11.2): every FL Studio and Bitwig id the readers in mo-formats use is
//! written down in `registry/*.json` with a name, and every id the registry says a reader relies on is used by one (or
//! is on a list of ones we know aren't used yet).
//!
//! How it works: the test reads the readers' own source files (`crates/mo-formats/src/fl` and `src/bitwig`) as plain
//! text and picks out the ids in them, so a new id in a reader is checked without anyone having to remember to add it
//! here. Most ids are named constants (`const EV_TEMPO: u8 = 156;`, `const F_START: u32 = 0x2af;`), and the name says
//! what kind of id it is (`EV_` an FL event, `VST_` a Fruity Wrapper record, `CLS_` a Bitwig class, `F_` a Bitwig
//! field). The few ids written as bare numbers, and the constants named some other way, are in the small tables below;
//! the test checks each one is still in its file, so the tables can't quietly go stale.
//!
//! Only the code before a file's `#[cfg(test)]` counts: ids made up in a reader's own tests aren't the reader's.
//! The registry and the sources are read from this repository (CARGO_MANIFEST_DIR), never from a user folder.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::{BTreeMap, BTreeSet};
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

/// Number constants in the reader files that aren't ids of the file format: versions of our own readers, colours,
/// values inside an event (what an automation link moves, a channel's type), and so on.
const NOT_IDS: &[&str] = &[
    // The readers' own version numbers (rule 7), not the file's.
    "VERSION",
    // FL: colours FL gives a track or channel that doesn't say.
    "DEFAULT_TRACK_COLOUR",
    "DEFAULT_CHANNEL_COLOUR",
    // FL: values inside an automation link (event 227), not events.
    "TARGET_VOLUME",
    "TARGET_PAN",
    "TARGET_PITCH",
    "TARGET_ON_OFF",
    "TARGET_PLUGIN",
    "TARGET_SLOT_ON_OFF",
    "TARGET_INSERT_VOLUME",
    "TARGET_INSERT_PAN",
    "TARGET_SONG",
    "TARGET_MAIN_VOLUME",
    "TARGET_TEMPO",
    "FL8_PLUGIN",
    // FL: values of a channel's type (event 21) and of the wrapper's kind (record 50).
    "CHANNEL_TYPE_INSTRUMENT",
    "CHANNEL_TYPE_AUTOMATION",
    "KIND_VST3",
    // FL: the wrapper version FL 7 and 8 save, and the playlist's pattern numbering (item above 20480).
    "OLD_WRAPPER",
    "PATTERN_BASE",
];

/// Ids the readers write as bare numbers rather than named constants: (file, which list, id). The test finds bare
/// Bitwig ids on lines that read or set a field or make an object, and FL VST3 records read with `get(&n)`; each one
/// it finds must be listed here, and each one listed here must still be in its file.
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

/// Bitwig metadata keys the readers use (by name, not number): (file, key). The test finds keys read with `meta(..)`,
/// `meta_str(..)` or `meta_strings(..)`; the curve writer's keys are listed by hand.
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

/// The reader files: (path from `crates/mo-formats/src`, the code before its tests).
fn sources() -> Vec<(String, String)> {
    let src = repo().join("crates/mo-formats/src");
    let mut files = Vec::new();
    for folder in ["fl", "bitwig"] {
        for entry in std::fs::read_dir(src.join(folder)).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path).unwrap();
                let code = text.split("#[cfg(test)]").next().unwrap_or_default().to_owned();
                files.push((format!("{folder}/{}", path.file_name().unwrap().to_string_lossy()), code));
            }
        }
    }
    files.sort();
    assert!(files.len() > 5, "the reader files weren't found under {}", src.display());
    files
}

/// The code's lines, leaving out comments (a `//` comment may mention ids the line doesn't use).
fn code_lines(code: &str) -> impl Iterator<Item = &str> {
    code.lines().map(|l| l.split("//").next().unwrap_or_default())
}

/// A number as Rust writes it: hex after "0x" or decimal, with any `_` between the digits.
fn number(text: &str) -> Option<u32> {
    let text = text.replace('_', "");
    match text.strip_prefix("0x") {
        Some(hex) => u32::from_str_radix(hex, 16).ok(),
        None => text.parse().ok(),
    }
}

/// Every `const NAME: u8 = n;` or `const NAME: u32 = n;` in the code: (name, n). Constants set from another constant
/// (`F_ITEMS = F_NOTES`) are left out; the one they copy is found where it's set.
fn number_constants(code: &str) -> Vec<(String, u32)> {
    let mut found = Vec::new();
    for line in code_lines(code) {
        let Some(rest) = line.trim().split("const ").nth(1) else { continue };
        let Some((name, rest)) = rest.split_once(':') else { continue };
        let Some((kind, value)) = rest.split_once('=') else { continue };
        if !matches!(kind.trim(), "u8" | "u32") {
            continue;
        }
        if let Some(n) = number(value.trim().trim_end_matches(';').trim()) {
            found.push((name.trim().to_owned(), n));
        }
    }
    found
}

/// Every Bitwig id written as a bare hex number where code reads or sets a field, makes an object or checks its class:
/// `field_obj(o, 0x2ae9)`, `p.num(0x2af)`, `root.set(0x362d, ..)`, `Object::new(0x108)`, `.class == 0xd90`. Only the
/// id itself counts, not a value that follows it (`Value::U8(0xff)`).
fn bare_bitwig_ids(code: &str) -> BTreeSet<u32> {
    // (what comes just before the id, whether the id is the call's second argument)
    const ID_CALLS: &[(&str, bool)] = &[
        ("field_obj(", true),
        ("field_list(", true),
        (".num(", false),
        (".str(", false),
        (".get(", false),
        (".has(", false),
        (".set(", false),
        ("Object::new(", false),
        (".class == ", false),
    ];
    let mut found = BTreeSet::new();
    for line in code_lines(code) {
        for &(call, second) in ID_CALLS {
            for (at, _) in line.match_indices(call) {
                let mut rest = &line[at + call.len()..];
                if second {
                    let Some((_, after)) = rest.split_once(',') else { continue };
                    rest = after;
                }
                let Some(hex) = rest.trim_start().strip_prefix("0x") else { continue };
                let digits: String = hex.chars().take_while(|c| c.is_ascii_hexdigit() || *c == '_').collect();
                if let Some(n) = number(&format!("0x{digits}")) {
                    found.insert(n);
                }
            }
        }
    }
    found
}

/// Every FL record read as `get(&n)` with a number n.
fn bare_fl_records(code: &str) -> BTreeSet<u32> {
    let mut found = BTreeSet::new();
    for line in code_lines(code) {
        for (at, _) in line.match_indices("get(&") {
            let digits: String = line[at + 5..].chars().take_while(char::is_ascii_digit).collect();
            if let Some(n) = number(&digits) {
                found.insert(n);
            }
        }
    }
    found
}

/// Every key read with `meta("..")`, `meta_str("..")` or `meta_strings("..")`.
fn meta_keys_read(code: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for call in ["meta(\"", "meta_str(\"", "meta_strings(\""] {
        for (at, _) in code.match_indices(call) {
            let rest = &code[at + call.len()..];
            found.insert(rest[..rest.find('"').unwrap()].to_owned());
        }
    }
    found
}

/// Whether the code has this number written as `0x..` (Bitwig) or as a plain number after `&` (FL records), as a whole
/// number rather than the start of a longer one.
fn has_number(code: &str, list: List, id: u32) -> bool {
    match list {
        FlVst3 => bare_fl_records(code).contains(&id),
        _ => code_lines(code).any(|line| {
            line.match_indices(&format!("{id:#x}"))
                .any(|(at, s)| !line[at + s.len()..].starts_with(|c: char| c.is_ascii_hexdigit() || c == '_'))
        }),
    }
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

/// Every id the readers use, found in their sources. Fails when it finds a number constant it can't place, or a bare
/// id the tables above don't list.
fn ids_in_use() -> Vec<Use> {
    let mut uses = Vec::new();
    let mut problems = Vec::new();
    for (file, code) in sources() {
        let fl = file.starts_with("fl/");
        for (name, id) in number_constants(&code) {
            let list = if fl && name.starts_with("EV_") {
                Some(FlEvent)
            } else if fl && name.starts_with("VST_") {
                Some(FlWrapper)
            } else if !fl && name.starts_with("CLS_") {
                Some(BwClass)
            } else if !fl && name.starts_with("F_") {
                Some(BwField)
            } else {
                OTHER_NAMED.iter().find(|(f, n, _)| *f == file && *n == name).map(|&(_, _, list)| list)
            };
            match list {
                Some(list) => uses.push(Use { file: file.clone(), constant: Some(name), list, id }),
                None if NOT_IDS.contains(&name.as_str()) => {}
                None => problems.push(format!(
                    "{file}: const {name} = {id} isn't known to this test: add it to OTHER_NAMED (an id) or NOT_IDS"
                )),
            }
        }

        let listed: BTreeSet<u32> = BARE.iter().filter(|(f, _, _)| *f == file).map(|&(_, _, id)| id).collect();
        let found = if fl { bare_fl_records(&code) } else { bare_bitwig_ids(&code) };
        for id in found.difference(&listed) {
            problems.push(format!("{file}: the bare id {id:#x} ({id}) isn't in this test's BARE table"));
        }
        for &(_, list, id) in BARE.iter().filter(|(f, _, _)| *f == file) {
            if has_number(&code, list, id) {
                uses.push(Use { file: file.clone(), constant: None, list, id });
            } else {
                problems.push(format!("{file} no longer has the id {id:#x} ({id}): take it off the BARE table"));
            }
        }
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

/// Every Bitwig metadata key the readers use is in the registry with a name, and the META table stays true.
#[test]
fn every_metadata_key_the_readers_use_is_in_the_registry() {
    let reg = Registries::read();
    let mut problems = Vec::new();
    let sources: BTreeMap<String, String> = sources().into_iter().collect();
    for (file, code) in &sources {
        for key in meta_keys_read(code) {
            if !META.contains(&(file.as_str(), key.as_str())) {
                problems.push(format!("{file}: the metadata key \"{key}\" isn't in this test's META table"));
            }
        }
    }
    for &(file, key) in META {
        if !sources[file].contains(&format!("\"{key}\"")) {
            problems.push(format!("{file} no longer has the metadata key \"{key}\": take it off META"));
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
    let code = "pub(super) const F_START: u32 = 0x2af;\nconst EV_TEMPO: u8 = 156;\nconst F_ITEMS: u32 = F_NOTES;\n\
                let x = doc.field_obj(o, 0x2ae9); // 0x9999 in a comment\nlet y = 0x1234;\nroot.set(0x362d, Value::U8(0xff));\nrecords.get(&3)\n";
    assert_eq!(number_constants(code), [("F_START".to_owned(), 0x2af), ("EV_TEMPO".to_owned(), 156)]);
    assert_eq!(bare_bitwig_ids(code), BTreeSet::from([0x2ae9, 0x362d]));
    assert_eq!(bare_fl_records(code), BTreeSet::from([3]));
    assert!(has_number(code, BwField, 0x2ae9) && !has_number(code, BwField, 0x2ae));
    assert_eq!(
        meta_keys_read("d.meta_str(\"title\") + d.meta(\"structure\")"),
        BTreeSet::from(["structure".into(), "title".into()])
    );
    assert_eq!(pascal_case("EV_PATTERN_NOTES"), "EvPatternNotes");
    assert_eq!(pascal_case("CLS_HARDWARE_IO"), "ClsHardwareIo");
}
