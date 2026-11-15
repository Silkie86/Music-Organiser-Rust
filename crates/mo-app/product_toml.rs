// Reads product.toml (at the top of the repo) for the build scripts of mo-app and mo-ui, which both take it in with
// `#[path]`. Only what that file needs: comment lines, blank lines and `key = "words"`. Anything else stops the build
// with the line it didn't understand, so a typo can't slip a wrong name into a release.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The file, from a crate's own folder (`crates/<name>`).
pub fn path(crate_dir: &Path) -> PathBuf {
    crate_dir.join("..").join("..").join("product.toml")
}

/// Every `key = "words"` in the file ([`parse`]). Stops the build on a line it can't read, a key given twice, a missing key, or
/// words with a `"` or `\` in them (they go into Rust, Slint and the installer as they are).
pub fn read(file: &Path) -> HashMap<String, String> {
    let text = std::fs::read_to_string(file).unwrap_or_else(|e| panic!("can't read {}: {e}", file.display()));
    let found = parse(&text).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
    for key in KEYS {
        assert!(found.contains_key(*key), "{}: `{key}` is missing", file.display());
    }
    found
}

/// The keys the app and the installer use. Every one must be there.
pub const KEYS: &[&str] = &["name", "description", "publisher", "website", "logo_png", "logo_ico", "installer_id"];

pub fn parse(text: &str) -> Result<HashMap<String, String>, String> {
    let mut found = HashMap::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let bad = || format!("line {}: expected key = \"words\", found `{line}`", n + 1);
        let (key, value) = line.split_once('=').ok_or_else(bad)?;
        let key = key.trim();
        let value = value.trim().strip_prefix('"').and_then(|v| v.strip_suffix('"')).ok_or_else(bad)?;
        if key.is_empty() || !key.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
            return Err(bad());
        }
        if value.contains(['"', '\\']) || value.trim().is_empty() {
            return Err(format!("line {}: `{key}` must be words without \" or \\", n + 1));
        }
        if found.insert(key.to_owned(), value.to_owned()).is_some() {
            return Err(format!("line {}: `{key}` is given twice", n + 1));
        }
    }
    Ok(found)
}
