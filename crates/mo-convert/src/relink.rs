//! A Bitwig project pointed back at the Library: Bitwig copies a Library multisample into the project's
//! `multi-samples` folder under a new name ("Kicks 1 Select-3-2-2.multisample") each time it saves, so a project
//! folder fills with byte-identical copies. This writes "<name> (fixed).bwproject" beside the project, with every
//! reference to a listed copy pointing at the Library file instead (Solomon's choice, 2026-10-31). The original and
//! its copies are never changed (rule 3); a copy may only go to the Recycle Bin once Solomon has opened the fixed
//! project and heard it play.
//!
//! A copy is referenced several ways: its bare name, a path relative to the project ("multi-samples/…"), an absolute
//! path where the project was saved (maybe on another drive since), the list of files the project uses in its
//! metadata, and beside them the Library path it was first copied from (maybe gone since). All of them point at the
//! Library file in the fixed project.
//!
//! A copy is only offered for the bin when nothing still names it: not the fixed project (anywhere, plugin settings
//! and the zip at the end included) and not another project in the same folder. The original still uses its copies;
//! it's kept as it is, so Solomon can go back to it while the copies are still there.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use mo_formats::bitwig::{self, Document, ObjId, Text, Value};
use mo_formats::paths;
use rayon::prelude::*;

use crate::save::save_checked;

/// The most of one zip entry that's looked through for a copy's name (a project's plugin settings are far smaller).
const LONGEST_ZIP_ENTRY: u64 = 256 << 20;

/// A copy in a project's `multi-samples` folder, and the Library file it's a byte-identical copy of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Relink {
    pub copy: PathBuf,
    pub library: PathBuf,
}

/// A fixed project on disk.
#[derive(Debug)]
pub struct Fixed {
    pub file: PathBuf,
    /// The copies nothing uses any more but the original project: only these may be offered for the Recycle Bin.
    pub changed: Vec<Relink>,
    /// Listed copies that are still needed, or that this project doesn't use, said plainly.
    pub left: Vec<String>,
}

/// A listed copy, as the project's texts are compared with it (without case or slash direction).
struct Copy {
    /// Its file name.
    name: String,
    /// Its bare name may stand for it: not when the Library file has the same name.
    bare: bool,
    /// The Library file, in full and its name.
    full: String,
    library_name: String,
    /// What's searched for in a file's bytes to tell whether it still names the copy.
    needles: Vec<Vec<u8>>,
}

/// How a text in the project leads to a file, compared without case or slash direction.
fn key(path: &str) -> String {
    path.replace('\\', "/").to_lowercase()
}

/// The file name at the end of a text either way round (a project saved on Windows, read anywhere).
fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// The copies, checked: each must be in this project's `multi-samples` folder, and its Library file must be there.
fn copies(project_dir: &Path, relinks: &[Relink]) -> Result<Vec<Copy>, String> {
    let folder = key(&project_dir.join("multi-samples").to_string_lossy());
    relinks
        .iter()
        .map(|r| {
            let copy = r.copy.to_string_lossy();
            let full = r.library.to_string_lossy().into_owned();
            let name = key(file_name(&copy));
            let in_folder = r.copy.parent().is_some_and(|p| key(&p.to_string_lossy()) == folder);
            if name.is_empty() || !in_folder {
                return Err(format!("{copy} isn't in this project's multi-samples folder"));
            }
            if !r.library.is_absolute() || !r.library.is_file() {
                return Err(format!("{full} isn't a Library file on disk, so nothing was written"));
            }
            let library_name = file_name(&full).to_owned();
            let bare = key(&library_name) != name;
            // Where the names match, only a path into the multi-samples folder is the copy.
            let forms = if bare { vec![name.clone()] } else { vec![format!("multi-samples/{name}")] };
            let mut needles = Vec::new();
            for form in forms.iter().chain(forms.iter().map(|f| f.replace('/', "\\")).collect::<Vec<_>>().iter()) {
                let form = form.to_lowercase();
                needles.push(form.as_bytes().to_vec());
                needles.push(form.encode_utf16().flat_map(u16::to_be_bytes).collect::<Vec<u8>>().to_ascii_lowercase());
                needles.push(form.encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<u8>>().to_ascii_lowercase());
            }
            needles.sort();
            needles.dedup();
            Ok(Copy { name, bare, full, library_name, needles })
        })
        .collect()
}

/// Which copy a text leads to: its bare name, "multi-samples/<name>", or an absolute path ending
/// "<this project's folder>/multi-samples/<name>" (where it was saved, maybe on another drive since).
fn copy_of(text: &str, project_folder: &str, copies: &[Copy]) -> Option<usize> {
    let k = key(text);
    copies.iter().position(|c| {
        let tail = format!("multi-samples/{}", c.name);
        (c.bare && k == c.name)
            || k == tail
            || (!project_folder.is_empty() && k.ends_with(&format!("/{project_folder}/{tail}")))
    })
}

/// Every text a value holds that may name a file: a text, a list of texts, or the text beside an object.
fn texts_mut(value: &mut Value) -> Vec<&mut Text> {
    match value {
        Value::Str(t) | Value::Named(_, t) => vec![t],
        Value::Strings(ts) => ts.iter_mut().collect(),
        _ => Vec::new(),
    }
}

/// The project's bytes with each listed copy pointed at its Library file, and which copies were changed. `project_dir`
/// is the project's folder, which relative paths start from. The encoder lays the file out again, so the zip at the
/// end and the offsets in the header stay right.
pub fn relink(raw: &[u8], project_dir: &Path, relinks: &[Relink]) -> Result<(Vec<u8>, Vec<bool>), String> {
    let copies = copies(project_dir, relinks)?;
    let mut doc = bitwig::decode(raw).map_err(|_| "not a Bitwig project".to_owned())?;
    if let Some(e) = &doc.body_error {
        return Err(format!("the project couldn't be read to the end ({e}), so it's left as it is"));
    }
    let folder = key(&project_dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
    let mut changed = vec![false; copies.len()];
    let point = |t: &mut Text, changed: &mut Vec<bool>| -> bool {
        let Some(c) = copy_of(t.as_str(), &folder, &copies) else { return false };
        let to = if t.as_str().contains(['/', '\\']) { &copies[c].full } else { &copies[c].library_name };
        *t = Text::new(to);
        changed[c] = true;
        true
    };
    // The metadata (the list of files the project uses).
    for (_, value) in &mut doc.meta {
        for t in texts_mut(value) {
            point(t, &mut changed);
        }
    }
    // The objects, and which of them held a copy.
    let mut held: Vec<ObjId> = Vec::new();
    for (i, o) in doc.objects.iter_mut().enumerate() {
        let mut here = false;
        for field in &mut o.fields {
            for t in texts_mut(&mut field.value) {
                here |= point(t, &mut changed);
            }
        }
        if here {
            held.push(ObjId(i as u32));
        }
    }
    // Beside each: the old Library path it came from, if it's gone, names the Library file now.
    let children = children(&doc);
    let mut on_disk: HashMap<String, bool> = HashMap::new();
    for id in held {
        for near in around(&doc, &children, id) {
            for field in &mut doc.obj_mut(near).fields {
                let Value::Str(t) = &mut field.value else { continue };
                let s = t.as_str();
                let Some(c) = copies.iter().find(|c| key(file_name(s)) == key(&c.library_name)) else {
                    continue;
                };
                if !s.contains(['/', '\\']) || key(s) == key(&c.full) || copy_of(s, &folder, &copies).is_some() {
                    continue;
                }
                let full = paths::full_path(project_dir, s);
                let there = *on_disk.entry(full.clone()).or_insert_with(|| Path::new(&full).is_file());
                if !there {
                    *t = Text::new(&c.full);
                }
            }
        }
    }
    let bytes = bitwig::encode(&doc, None)?;
    Ok((bytes, changed))
}

/// Each object's children, found once.
fn children(doc: &Document) -> Vec<Vec<ObjId>> {
    let mut out = vec![Vec::new(); doc.objects.len()];
    for (i, o) in doc.objects.iter().enumerate() {
        if let Some((p, _)) = o.parent
            && let Some(list) = out.get_mut(p.0 as usize)
        {
            list.push(ObjId(i as u32));
        }
    }
    out
}

/// The objects around one: itself, its children, its parent and the parent's other children (where Bitwig keeps a
/// file's name, its location and where it came from).
fn around(doc: &Document, children: &[Vec<ObjId>], of: ObjId) -> Vec<ObjId> {
    let mut ids = vec![of];
    let parent = doc.obj(of).parent.map(|(p, _)| p);
    ids.extend(parent);
    for p in [Some(of), parent].into_iter().flatten() {
        for &c in children.get(p.0 as usize).into_iter().flatten() {
            if !ids.contains(&c) {
                ids.push(c);
            }
        }
    }
    ids
}

/// Which copies a file's bytes still name anywhere: its texts (in either text form Bitwig writes), plugin settings,
/// and the zip at the end, unpacked. Only ASCII letters are compared without case, so this errs towards "still named".
fn named_in(raw: &[u8], copies: &[Copy]) -> Vec<bool> {
    let mut named = vec![false; copies.len()];
    let mut look = |bytes: &[u8]| {
        let lower = bytes.to_ascii_lowercase();
        for (c, copy) in copies.iter().enumerate() {
            named[c] |= copy.needles.iter().any(|n| !n.is_empty() && lower.windows(n.len()).any(|w| w == n.as_slice()));
        }
    };
    look(raw);
    let zip = bitwig::decode(raw).ok().and_then(|d| d.zip().map(<[u8]>::to_vec));
    if let Some(Ok(mut archive)) = zip.map(|z| zip::ZipArchive::new(Cursor::new(z))) {
        for i in 0..archive.len() {
            let mut entry = Vec::new();
            if let Ok(e) = archive.by_index(i)
                && e.take(LONGEST_ZIP_ENTRY).read_to_end(&mut entry).is_ok()
            {
                look(&entry);
            }
        }
    }
    named
}

/// Where the fixed project goes: "<name> (fixed).bwproject" beside it, or "(fixed 2)" and on while that's taken, so
/// nothing already there is replaced. None when all are taken.
pub fn fixed_file(project: &Path) -> Option<PathBuf> {
    fixed_names(project).find(|f| !f.exists())
}

fn fixed_names(project: &Path) -> impl Iterator<Item = PathBuf> + '_ {
    let stem = paths::stem(&project.to_string_lossy()).to_owned();
    (1..1000).map(move |n| {
        let mark = if n == 1 { "fixed".to_owned() } else { format!("fixed {n}") };
        project.with_file_name(format!("{stem} ({mark}).bwproject"))
    })
}

/// Writes the fixed project beside `project` (copy, check, then rename, never over a file that's there), and says
/// which copies nothing uses any more but the original.
pub fn fix_bitwig_project(project: &Path, relinks: &[Relink]) -> Result<Fixed, String> {
    let dir = project.parent().ok_or("no folder")?;
    let raw = std::fs::read(project).map_err(|e| format!("Couldn't read {}: {e}", project.display()))?;
    let (bytes, mut changed) = relink(&raw, dir, relinks)?;
    if !changed.contains(&true) {
        return Err("the project uses none of these copies, so nothing was written".to_owned());
    }
    let copies = copies(dir, relinks)?;
    let mut left = Vec::new();
    let name = |c: usize| file_name(&relinks[c].copy.to_string_lossy()).to_owned();
    // Still named in the fixed project somewhere this doesn't change (a plugin's own settings).
    for (c, still) in named_in(&bytes, &copies).into_iter().enumerate() {
        if !changed[c] {
            left.push(format!("{}: this project doesn't use it, so it's left alone", name(c)));
        } else if still {
            changed[c] = false;
            left.push(format!("{}: the project still names it somewhere that isn't changed, so it's kept", name(c)));
        }
    }
    if !changed.contains(&true) {
        return Err(format!("every copy is still needed, so nothing was written: {}", left.join("; ")));
    }
    // Other projects in the same folder may use the same copies.
    let others = other_projects(project);
    let used: Vec<(String, Vec<bool>)> = others
        .par_iter()
        .map(|p| {
            let shown = file_name(&p.to_string_lossy()).to_owned();
            match std::fs::read(p) {
                Ok(raw) if bitwig::decode(&raw).is_ok() => (shown, named_in(&raw, &copies)),
                // One that can't be read might use any of them.
                _ => (shown, vec![true; copies.len()]),
            }
        })
        .collect();
    for c in 0..copies.len() {
        if let Some((other, _)) = used.iter().find(|(_, u)| changed[c] && u[c]) {
            changed[c] = false;
            left.push(format!("{}: {other} in the same folder may still use it, so it's kept", name(c)));
        }
    }

    // The fixed project must read back cleanly, and name none of the copies it was meant to drop.
    let dropped: Vec<usize> = (0..copies.len()).filter(|&c| changed[c]).collect();
    let problem = |back: &[u8]| match bitwig::decode(back) {
        Ok(doc) if doc.body_error.is_none() => {
            let named = named_in(back, &copies);
            dropped.iter().any(|&c| named[c]).then(|| "it still points at a copy".to_owned())
        }
        Ok(doc) => doc.body_error,
        Err(_) => Some("not a Bitwig project".to_owned()),
    };
    let file = save_new(project, &bytes, problem)?;
    let changed = relinks.iter().zip(&changed).filter(|(_, c)| **c).map(|(r, _)| r.clone()).collect();
    Ok(Fixed { file, changed, left })
}

/// The other Bitwig projects beside `project` (an earlier fixed one included: it may still name a copy it kept).
fn other_projects(project: &Path) -> Vec<PathBuf> {
    let Some(dir) = project.parent() else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p != project && p.is_file())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("bwproject")))
        .collect()
}

/// Saves the fixed project under the first free "(fixed …)" name. The name is claimed first (an empty file made only
/// if nothing's there), so a file that appears meanwhile is never replaced; the checked save then goes over the claim.
fn save_new(project: &Path, bytes: &[u8], problem: impl Fn(&[u8]) -> Option<String>) -> Result<PathBuf, String> {
    for file in fixed_names(project) {
        match OpenOptions::new().write(true).create_new(true).open(&file) {
            Ok(claim) => {
                drop(claim);
                return match save_checked(&file, bytes, &problem) {
                    Ok(()) => Ok(file),
                    Err(e) => {
                        let _ = std::fs::remove_file(&file);
                        Err(e)
                    }
                };
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("Couldn't save {}: {e}", file.display())),
        }
    }
    Err("every \"(fixed …)\" name up to 999 is taken, so nothing was written".to_owned())
}
