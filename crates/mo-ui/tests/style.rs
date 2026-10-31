//! The style check (spec 11.2): pages use the shared style in `studio.slint` and never set a raw colour, a text size,
//! gap or padding of their own, give a button a look of its own or draw their own play and stop signs. Only `studio.slint` may hold colour values and the looks
//! that change under the mouse.

// Test files may unwrap: a test that stops there has failed, which is what it should do.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

fn slint_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("the ui folder is readable") {
        let path = entry.expect("a ui file").path();
        if path.is_dir() {
            slint_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "slint") {
            out.push(path);
        }
    }
}

/// The code on a line: what's before a `//` comment, with quoted text blanked out.
fn code_of(line: &str) -> String {
    let code = line.split("//").next().unwrap_or_default();
    let mut quoted = false;
    code.chars()
        .map(|c| {
            if c == '"' {
                quoted = !quoted;
            }
            if quoted { ' ' } else { c }
        })
        .collect()
}

/// A colour value: a `#` followed by 3, 4, 6 or 8 hex digits and then a word boundary, `rgb(`, `rgba(`, `hsv(` or one
/// of Slint's named `Colors`.
fn has_raw_colour(line: &str) -> bool {
    let code = code_of(line);
    if ["rgb(", "rgba(", "hsv(", "Colors."].iter().any(|c| code.contains(c)) {
        return true;
    }
    let bytes = code.as_bytes();
    bytes.iter().enumerate().any(|(i, b)| {
        if *b != b'#' {
            return false;
        }
        let digits = bytes[i + 1..].iter().take_while(|c| c.is_ascii_hexdigit()).count();
        let ends = bytes.get(i + 1 + digits).is_none_or(|c| !c.is_ascii_alphanumeric());
        matches!(digits, 3 | 4 | 6 | 8) && ends
    })
}

/// A text size, gap or padding set as a number ("font-size: 13px", "spacing: 6px") rather than a shared token. Gives
/// what it is.
fn own_size(code: &str) -> Option<&'static str> {
    let props: [(&str, &'static str); 7] = [
        ("font-size", "text size"),
        ("spacing", "gap"),
        ("padding", "padding"),
        ("padding-left", "padding"),
        ("padding-right", "padding"),
        ("padding-top", "padding"),
        ("padding-bottom", "padding"),
    ];
    let code = code.trim_start();
    let (name, value) = code.split_once(':')?;
    let what = props.iter().find(|(p, _)| *p == name.trim())?.1;
    // A number with a unit; "0" (no gap) and tokens are fine.
    let value = value.trim().trim_end_matches(';').trim();
    let number: String = value.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    number.parse::<f64>().is_ok_and(|n| n > 0.0).then_some(what)
}

#[test]
fn a_size_of_its_own_is_found() {
    assert_eq!(own_size("    font-size: 13px;"), Some("text size"));
    assert_eq!(own_size("spacing: 6px;"), Some("gap"));
    assert_eq!(own_size("padding-left: 14px;"), Some("padding"));
    assert_eq!(own_size("font-size: Type.body;"), None);
    assert_eq!(own_size("padding: 0;"), None);
    assert_eq!(own_size("padding: 0px;"), None);
    assert_eq!(own_size("letter-spacing: 0.8px;"), None);
    assert_eq!(own_size("width: 20px;"), None);
}

#[test]
fn pages_use_the_shared_style() {
    let ui = Path::new(env!("CARGO_MANIFEST_DIR")).join("ui");
    let mut files = Vec::new();
    slint_files(&ui, &mut files);
    assert!(!files.is_empty());
    let mut problems = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(&file).expect("a readable .slint file");
        let shared = file.file_name().is_some_and(|n| n == "studio.slint");
        for (n, line) in text.lines().enumerate() {
            let at = format!("{}:{}", file.strip_prefix(&ui).unwrap_or(&file).display(), n + 1);
            if !shared && has_raw_colour(line) {
                problems.push(format!("{at}: a raw colour; use a Theme token from studio.slint"));
            }
            // A look that changes under the mouse or when pressed is a button's: the shared parts have those.
            let code = code_of(line);
            if !shared && (code.contains("has-hover") || code.contains(".pressed")) {
                problems.push(format!("{at}: a button look of its own; use or add a shared part in studio.slint"));
            }
            if !shared && let Some(what) = own_size(&code) {
                problems.push(format!("{at}: a {what} of its own; use a Type or Space token from studio.slint"));
            }
            if line.contains('▶') || line.contains('■') {
                problems.push(format!("{at}: a typed play or stop sign; use the shared PlayIcon"));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn raw_colours_are_spotted() {
    assert!(has_raw_colour("background: #121212;"));
    assert!(has_raw_colour("color: #fff;"));
    assert!(!has_raw_colour("// #121212 in a comment"));
    assert!(!has_raw_colour("text: \"#1 hit\";"));
    assert!(has_raw_colour("background: rgb(255, 0, 0);"));
    assert!(has_raw_colour("background: Colors.red;"));
    assert!(has_raw_colour("color: hsv(0, 1, 1);"));
    assert!(!has_raw_colour("text: \"Colors.red\";"));
}

/// The launcher is read by Windows PowerShell 5.1, which misreads anything but plain ASCII in a file without a BOM.
#[test]
fn launcher_scripts_are_plain_ascii() {
    let dev = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/dev");
    let mut checked = 0;
    for entry in std::fs::read_dir(&dev).expect("tools/dev is there") {
        let path = entry.expect("listable").path();
        if path.is_dir() {
            continue;
        }
        let text = std::fs::read(&path).expect("readable");
        if let Some(at) = text.iter().position(|b| !b.is_ascii()) {
            panic!("{} has a non-ASCII byte at {at}", path.display());
        }
        checked += 1;
    }
    assert!(checked >= 2);
}
