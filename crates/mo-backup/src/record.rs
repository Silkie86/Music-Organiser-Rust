//! The backup's own record of what it holds: for each file on the backup, the size and time the file in your folder
//! had when it was copied, the time the copy itself had once placed, and the SHA-256 of its bytes. It lives on the
//! backup drive itself (`<backup>\.mo-backup\record.tsv`, one plain line per file), so it goes wherever the drive goes
//! and never touches the app's databases.
//!
//! It does two jobs. A file is up to date without reading it when both it and its copy are as the record says (a copy
//! that shares its bytes with another file can't take its own file's time, so the time alone can't say). And a new
//! file whose bytes are already on the backup can share them: the record only says where to look, and the bytes are
//! compared before anything is shared. A line that doesn't make sense is skipped; a record that can't be read at all
//! is left as it is, not written over.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// The folder on the backup that holds the record.
pub const FOLDER: &str = ".mo-backup";
const FILE: &str = "record.tsv";

/// What was copied to one place on the backup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Where it is on the backup, relative, with forward slashes, as written.
    pub path: String,
    pub size: u64,
    /// The source's time when it was copied, in nanoseconds since 1970.
    pub time: i128,
    /// The copy's own time once placed.
    pub copy_time: i128,
    pub hash: [u8; 32],
}

/// The record, keyed by place on the backup (small letters on a drive that ignores letter case).
#[derive(Clone, Debug, Default)]
pub struct Record {
    entries: HashMap<String, Entry>,
    /// The keys of the entries of each size, to find files to share without looking at every entry.
    by_size: HashMap<u64, Vec<String>>,
    changed: bool,
}

/// A time as the record keeps it.
pub fn nanos(t: SystemTime) -> i128 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => i128::try_from(d.as_nanos()).unwrap_or(i128::MAX),
        Err(e) => -i128::try_from(e.duration().as_nanos()).unwrap_or(i128::MAX),
    }
}

impl Record {
    /// Reads the record from a backup. None there yet: an empty record. One that can't be read (a failing drive, a
    /// program holding it): an error, and the run goes on without one and leaves it as it is.
    pub fn load(backup: &Path, ignores_case: bool) -> io::Result<Record> {
        let bytes = match fs::read(backup.join(FOLDER).join(FILE)) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Record::default()),
            Err(e) => return Err(e),
        };
        let mut record = Record::default();
        for line in String::from_utf8_lossy(&bytes).lines() {
            let mut parts = line.splitn(5, '\t');
            let (Some(hash), Some(size), Some(time), Some(copy_time), Some(path)) =
                (parts.next(), parts.next(), parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            let (Some(hash), Ok(size), Ok(time), Ok(copy_time)) =
                (unhex(hash), size.parse(), time.parse(), copy_time.parse())
            else {
                continue;
            };
            if safe_rel(path) {
                let entry = Entry { path: path.to_owned(), size, time, copy_time, hash };
                record.insert(fold(path, ignores_case), entry);
            }
        }
        record.changed = false;
        Ok(record)
    }

    /// Writes the record to the backup when anything in it changed: under a temporary name first, then renamed, so a
    /// run that stops half way leaves the old record whole.
    pub fn save(&self, backup: &Path) -> io::Result<()> {
        if !self.changed {
            return Ok(());
        }
        let folder = backup.join(FOLDER);
        fs::create_dir_all(&folder)?;
        let part = folder.join(format!("{FILE}.mo-part"));
        let mut lines: Vec<&Entry> = self.entries.values().collect();
        lines.sort_by(|a, b| a.path.cmp(&b.path));
        let mut out = io::BufWriter::new(fs::File::create(&part)?);
        for e in lines {
            writeln!(out, "{}\t{}\t{}\t{}\t{}", hex(&e.hash), e.size, e.time, e.copy_time, e.path)?;
        }
        out.into_inner().map_err(io::IntoInnerError::into_error)?.sync_all()?;
        fs::rename(&part, folder.join(FILE))
    }

    pub fn get(&self, key: &str) -> Option<&Entry> {
        self.entries.get(key)
    }

    /// Records a file. A name with a tab or a line break in it isn't recorded (the record is one line per file): it's
    /// only read again next time.
    pub fn set(&mut self, key: String, entry: Entry) {
        if entry.path.contains(['\t', '\n', '\r']) || !safe_rel(&entry.path) {
            return;
        }
        if self.entries.get(&key) != Some(&entry) {
            self.insert(key, entry);
        }
    }

    fn insert(&mut self, key: String, entry: Entry) {
        let size = entry.size;
        if let Some(old) = self.entries.insert(key.clone(), entry)
            && let Some(keys) = self.by_size.get_mut(&old.size)
        {
            keys.retain(|k| k != &key);
        }
        self.by_size.entry(size).or_default().push(key);
        self.changed = true;
    }

    /// Up to `most` places on the backup said to hold bytes of this size, to compare and share: one of each different
    /// hash first, then more of each, so neither one gone place nor many copies of one sound crowd out the rest.
    /// `except` (the file's own place) is left out.
    pub fn same_size(&self, size: u64, except: &str, most: usize) -> Vec<String> {
        let mut groups: Vec<([u8; 32], Vec<&str>)> = Vec::new();
        for key in self.by_size.get(&size).into_iter().flatten() {
            if key == except {
                continue;
            }
            let Some(e) = self.entries.get(key) else { continue };
            match groups.iter_mut().find(|(h, _)| *h == e.hash) {
                Some((_, places)) => places.push(&e.path),
                None => groups.push((e.hash, vec![&e.path])),
            }
        }
        let mut out = Vec::new();
        for round in 0.. {
            let before = out.len();
            for (_, places) in &groups {
                if let Some(p) = places.get(round) {
                    out.push((*p).to_owned());
                    if out.len() >= most {
                        return out;
                    }
                }
            }
            if out.len() == before {
                break;
            }
        }
        out
    }
}

/// A place on the backup as written (relative, forward slashes), or `None` when `dest` isn't inside the backup.
pub fn rel_path(backup: &Path, dest: &Path) -> Option<String> {
    let rel = dest.strip_prefix(backup).ok()?;
    let text = rel.to_string_lossy();
    // Only Windows uses "\\" between folders; elsewhere it can be part of a name, which the record can't hold.
    let text = if cfg!(windows) { text.replace('\\', "/") } else { text.into_owned() };
    safe_rel(&text).then_some(text)
}

/// The key a place is recorded under.
pub fn fold(rel: &str, ignores_case: bool) -> String {
    if ignores_case { rel.to_lowercase() } else { rel.to_owned() }
}

/// The file a recorded place names on the backup; `None` for a place that would lead outside it (a damaged line).
pub fn place(backup: &Path, rel: &str) -> Option<PathBuf> {
    safe_rel(rel).then(|| rel.split('/').fold(backup.to_path_buf(), |p, part| p.join(part)))
}

/// Only plain names, so a place never leads outside the backup ("..", a drive, a leading slash).
fn safe_rel(rel: &str) -> bool {
    !rel.is_empty()
        && rel.split('/').all(|part| {
            !part.is_empty() && {
                let mut c = Path::new(part).components();
                matches!((c.next(), c.next()), (Some(Component::Normal(_)), None))
            }
        })
}

fn hex(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 || !text.is_ascii() {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(text.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}
