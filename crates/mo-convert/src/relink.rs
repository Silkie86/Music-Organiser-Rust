//! A Bitwig project pointed back at the Library: Bitwig copies a Library multisample into the project's
//! `multi-samples` folder under a new name ("Kicks 1 Select-3-2-2.multisample") each time it saves, so a project
//! folder fills with byte-identical copies. This writes "<name> (fixed).bwproject" beside the project, with every
//! reference to a listed copy pointing at the Library file instead (Solomon's choice, 2026-10-31). The original and
//! its copies are never changed (rule 3); a copy may only go to the Recycle Bin once Solomon has opened the fixed
//! project and heard it play.
//!
//! A copy is referenced several ways, in the object that holds the file and the objects around it: its bare name,
//! a path relative to the project ("multi-samples/…"), an absolute path where the project was saved (maybe on another
//! drive since), and the Library path it was first copied from (maybe gone since). The names and paths that lead to a
//! listed copy, and the old Library path beside them, all point at the Library file in the fixed project.

use std::path::{Path, PathBuf};

use mo_formats::bitwig::{self, Document, ObjId, Text, Value};

use crate::save::save_checked;

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
    /// The copies the fixed project no longer uses: only these may be offered for the Recycle Bin.
    pub changed: Vec<Relink>,
    /// Listed copies the project still uses in a way this didn't change, or doesn't mention at all, said plainly.
    pub left: Vec<String>,
}

/// How a text in the project leads to a file, compared without case or slash direction.
fn key(path: &str) -> String {
    path.replace('\\', "/").to_lowercase()
}

/// The file name at the end of a path either way round.
fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// Which listed copy a text leads to: its bare name, or a path ending in "multi-samples/<name>" (relative, or absolute
/// on any drive).
fn copy_of(text: &str, copies: &[(String, String)]) -> Option<usize> {
    let k = key(text);
    copies.iter().position(|(name, tail)| k == *name || k == *tail || k.ends_with(&format!("/{tail}")))
}

/// The objects around one: itself, its parent, and the parent's other children (where Bitwig keeps a file's name,
/// its location and where it came from).
fn around(doc: &Document, of: ObjId) -> Vec<ObjId> {
    let parent = doc.obj(of).parent.map(|(p, _)| p);
    let mut ids = vec![of];
    ids.extend(parent);
    for (i, o) in doc.objects.iter().enumerate() {
        let id = ObjId(i as u32);
        let near = o.parent.is_some_and(|(p, _)| Some(p) == parent || p == of);
        if near && !ids.contains(&id) {
            ids.push(id);
        }
    }
    ids
}

/// The project's bytes with each listed copy pointed at its Library file, and which copies were changed. The
/// encoder lays the file out again, so the zip at the end and the offsets in the header stay right.
pub fn relink(raw: &[u8], relinks: &[Relink]) -> Result<(Vec<u8>, Vec<bool>), String> {
    let mut doc = bitwig::decode(raw).map_err(|_| "not a Bitwig project".to_owned())?;
    if let Some(e) = &doc.body_error {
        return Err(format!("the project couldn't be read to the end ({e}), so it's left as it is"));
    }
    let copies: Vec<(String, String)> = relinks
        .iter()
        .map(|r| {
            let name = key(file_name(&r.copy.to_string_lossy()));
            (name.clone(), format!("multi-samples/{name}"))
        })
        .collect();
    let library: Vec<(String, String)> = relinks
        .iter()
        .map(|r| {
            let full = r.library.to_string_lossy().into_owned();
            (file_name(&full).to_owned(), full)
        })
        .collect();

    // Where each copy is mentioned.
    let mut found: Vec<(ObjId, usize, usize)> = Vec::new();
    for (i, o) in doc.objects.iter().enumerate() {
        for (f, field) in o.fields.iter().enumerate() {
            if let Value::Str(t) = &field.value
                && let Some(c) = copy_of(t.as_str(), &copies)
            {
                found.push((ObjId(i as u32), f, c));
            }
        }
    }
    let mut changed = vec![false; relinks.len()];
    for &(id, f, c) in &found {
        let (name, full) = library.get(c).ok_or("no such copy")?;
        let old = match &doc.obj(id).fields.get(f).map(|x| &x.value) {
            Some(Value::Str(t)) => t.as_str().to_owned(),
            _ => continue,
        };
        let new = if old.contains(['/', '\\']) { full.clone() } else { name.clone() };
        set_text(&mut doc, id, f, &new);
        changed[c] = true;
        // The old Library path kept beside it, if it's gone: the Library file it names now.
        for near in around(&doc, id) {
            let fields = doc.obj(near).fields.len();
            for g in 0..fields {
                let Some(Value::Str(t)) = doc.obj(near).fields.get(g).map(|x| &x.value) else { continue };
                let t = t.as_str().to_owned();
                let elsewhere = t.contains(['/', '\\']) && copy_of(&t, &copies).is_none() && key(&t) != key(full);
                if elsewhere && key(file_name(&t)) == key(name) && !Path::new(&t).is_file() {
                    set_text(&mut doc, near, g, full);
                }
            }
        }
    }
    let zip = doc.zip().map(<[u8]>::to_vec);
    let bytes = bitwig::encode(&doc, zip.as_deref())?;
    Ok((bytes, changed))
}

fn set_text(doc: &mut Document, id: ObjId, field: usize, text: &str) {
    if let Some(f) = doc.obj_mut(id).fields.get_mut(field) {
        f.value = Value::Str(Text::new(text));
    }
}

/// Where the fixed project goes: "<name> (fixed).bwproject" beside it, or "(fixed 2)" and on while that's taken, so
/// nothing already there is replaced.
pub fn fixed_file(project: &Path) -> PathBuf {
    let stem = project.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    (1..1000)
        .map(|n| {
            let mark = if n == 1 { "fixed".to_owned() } else { format!("fixed {n}") };
            project.with_file_name(format!("{stem} ({mark}).bwproject"))
        })
        .find(|f| !f.exists())
        .unwrap_or_else(|| project.with_file_name(format!("{stem} (fixed).bwproject")))
}

/// Writes the fixed project beside `project` (copy, check, then rename), and says which copies it no longer uses.
pub fn fix_bitwig_project(project: &Path, relinks: &[Relink]) -> Result<Fixed, String> {
    let raw = std::fs::read(project).map_err(|e| format!("Couldn't read {}: {e}", project.display()))?;
    let (bytes, mut changed) = relink(&raw, relinks)?;
    if !changed.contains(&true) {
        return Err("none of the copies are mentioned in the project, so nothing was written".to_owned());
    }
    let mut left = Vec::new();
    for (r, c) in relinks.iter().zip(changed.iter_mut()) {
        let name = file_name(&r.copy.to_string_lossy()).to_owned();
        if !*c {
            left.push(format!("{name} isn't mentioned in the project, so it's left alone"));
        } else if bytes.windows(name.len()).any(|w| w.eq_ignore_ascii_case(name.as_bytes())) {
            // Still named somewhere this doesn't change (a plugin's own settings): the copy is still needed.
            *c = false;
            left.push(format!("{name} is still mentioned in the project, so its copy is still needed"));
        }
    }
    if !changed.contains(&true) {
        return Err(format!("every copy is still needed, so nothing was written: {}", left.join("; ")));
    }
    // The fixed project must read back, and mention no listed copy any more.
    let copies: Vec<(String, String)> = relinks
        .iter()
        .map(|r| {
            let name = key(file_name(&r.copy.to_string_lossy()));
            (name.clone(), format!("multi-samples/{name}"))
        })
        .collect();
    let file = fixed_file(project);
    save_checked(&file, &bytes, |back| match bitwig::decode(back) {
        Ok(doc) if doc.body_error.is_none() => doc
            .objects
            .iter()
            .flat_map(|o| &o.fields)
            .any(|f| matches!(&f.value, Value::Str(t) if copy_of(t.as_str(), &copies).is_some()))
            .then(|| "it still points at a copy".to_owned()),
        Ok(doc) => doc.body_error,
        Err(_) => Some("not a Bitwig project".to_owned()),
    })?;
    let changed = relinks.iter().zip(&changed).filter(|(_, c)| **c).map(|(r, _)| r.clone()).collect();
    Ok(Fixed { file, changed, left })
}
