//! A Bitwig project pointed back at the Library: Bitwig copies a Library multisample into the project's
//! `multi-samples` folder under a new name ("Kicks 1 Select-3-2-2.multisample") each time it saves, so a project
//! folder fills with byte-identical copies. This writes "<name> (fixed).bwproject" beside the project, with every
//! reference to a listed copy pointing at the Library file instead (Solomon's choice, 2026-10-31). The original and
//! its copies are never changed (rule 3); a copy may only go to the Recycle Bin once Solomon has opened the fixed
//! project and heard it play.
//!
//! A copy is referenced several ways: its bare name, a path relative to the project ("multi-samples/…"), an absolute
//! path where the project was saved (maybe on another drive since), the list of files the project uses in its
//! metadata, and beside them the Library path it was first copied from (maybe gone since). In the fixed project the
//! copy's sample file is written as Bitwig writes a Library file it didn't copy (Solomon's neon rmx 40, 2026-11-11:
//! the first fixed projects put the Library path where Bitwig keeps a path inside the project's folder, so Bitwig
//! showed the multisamples as missing): nothing inside the project's folder, the Library file's name, and every path
//! it's found by the Library file. The used files list only lists files inside the project's folder, so the copy
//! leaves it.
//!
//! While it's at it, the fixed project finds files whose folder has moved since the project was saved (Bitwig
//! Studio's folder moved from I: to K:, or its Library from Documents): each path of a sample file that's gone is
//! pointed at the same file in the moved folder, or in Bitwig's Library now, when one is there with the same name and
//! size (rule 13: the project's and the DAW's own folders only).
//!
//! A copy is only offered for the bin when it's still the same as its Library file and nothing else still names it:
//! not the fixed project (anywhere, plugin settings in the zip at the end included) and no other Bitwig file in the
//! project's folder or below it (Bitwig's auto-backups too). Anything that can't be looked through counts as naming
//! every copy. The original still uses its copies; it's kept as it is, so Solomon can go back to it while the copies
//! are still there.

use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use memchr::memmem;
use mo_formats::bitwig::{self, Document, ObjId, Text, Value};
use mo_formats::{case, paths};
use rayon::prelude::*;

use crate::save::save_checked_new;

/// A sample file (a multisample, wavetable or sample a device uses): 0x512 its name, 0xd3a where it is inside the
/// project's folder ("" when it isn't in it), 0xd3b its file reference (0x2a, the path in 0x3b) that Bitwig finds it
/// by when it isn't inside, 0x15 the files it was found at before (0xc5b, a list of file references), 0x17 its size.
const SAMPLE_FILE: u32 = 0x4a6;
const F_NAME: u32 = 0x512;
const F_IN_PROJECT: u32 = 0xd3a;
const F_FILE: u32 = 0xd3b;
const F_FOUND: u32 = 0x15;
const F_FOUND_FILES: u32 = 0xc5b;
const F_SIZE: u32 = 0x17;
/// A file reference's path.
const F_PATH: u32 = 0x3b;

/// The Bitwig files that can name a multisample.
const BITWIG_FILES: [&str; 3] = ["bwproject", "bwclip", "bwpreset"];

/// The most of one zip entry that's looked through (as mo-formats' readers allow): a damaged zip can say a small
/// file unpacks to gigabytes. One that's bigger can't be looked through, so it counts as naming every copy.
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
    /// Files the project named in a folder that has moved since (Bitwig Studio's, to another drive), pointed at where
    /// they are now: "Basic Shapes.wt is now at K:\Bitwig Studio\Library\…".
    pub found: Vec<String>,
}

/// A listed copy, as the project's texts are compared with it.
struct Listed {
    /// Its file name, as shown.
    shown: String,
    /// Its bare name may stand for it: not when the Library file has the same name.
    bare: bool,
    /// [`key`] of its file name, and of "multi-samples/<name>".
    name: String,
    tail: String,
    /// The Library file, in full and its name.
    full: String,
    library_name: String,
    /// What's searched for in a file's bytes (made small in ASCII) to tell whether it still names the copy.
    needles: Vec<Vec<u8>>,
}

/// How a text in the project leads to a file, compared as Windows compares names (case aside, either slash).
fn key(path: &str) -> String {
    case::key(&path.replace('\\', "/"))
}

/// The file name at the end of a text either way round: a project saved on Windows names its files with `\` on every
/// system (unlike `paths::file_name`, which only splits there on Windows).
fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// The ways a name may be written in a file's bytes: as UTF-8 and either way round as UTF-16, in its own case, small
/// and capital letters, escaped as a URL or XML would, all made small in ASCII as the searched bytes are.
fn needles(forms: &[String]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for form in forms {
        let cases = [form.clone(), case::to_lower(form), case::key(form), form.to_lowercase(), form.to_uppercase()];
        for c in cases {
            let escaped = [c.replace(' ', "%20"), c.replace('&', "&amp;"), c.replace('\\', "\\\\")];
            for t in std::iter::once(c).chain(escaped) {
                out.push(t.as_bytes().to_ascii_lowercase());
                out.push(t.encode_utf16().flat_map(u16::to_be_bytes).collect::<Vec<u8>>().to_ascii_lowercase());
                out.push(t.encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<u8>>().to_ascii_lowercase());
            }
        }
    }
    out.retain(|n| !n.is_empty());
    out.sort();
    out.dedup();
    out
}

/// The copies, checked: each must be in this project's `multi-samples` folder, and its Library file must be there.
fn listed(project_dir: &Path, relinks: &[Relink]) -> Result<Vec<Listed>, String> {
    let folder = key(&project_dir.join("multi-samples").to_string_lossy());
    relinks
        .iter()
        .map(|r| {
            let copy = r.copy.to_string_lossy();
            let full = r.library.to_string_lossy().into_owned();
            let shown = r.copy.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if shown.is_empty() {
                return Err(format!("{copy} has no file name, so nothing was written"));
            }
            if !r.copy.parent().is_some_and(|p| key(&p.to_string_lossy()) == folder) {
                return Err(format!("{copy} isn't in this project's multi-samples folder, so nothing was written"));
            }
            if !r.library.is_absolute() || !r.library.is_file() {
                return Err(format!("{full} isn't a Library file on disk, so nothing was written"));
            }
            let library_name = file_name(&full).to_owned();
            let name = key(&shown);
            let bare = key(&library_name) != name;
            // Where the names match, only a path into the multi-samples folder is the copy.
            let forms = if bare {
                vec![shown.clone()]
            } else {
                vec![format!("multi-samples/{shown}"), format!("multi-samples\\{shown}")]
            };
            Ok(Listed {
                needles: needles(&forms),
                tail: format!("MULTI-SAMPLES/{name}"),
                shown,
                bare,
                name,
                full,
                library_name,
            })
        })
        .collect()
}

/// Which copy a text leads to: its bare name, "multi-samples/<name>", or an absolute path ending
/// "<this project's folder>/multi-samples/<name>" (where it was saved, maybe on another drive since).
fn copy_of(text: &str, project_folder: &str, listed: &[Listed]) -> Option<usize> {
    let k = key(text);
    listed.iter().position(|c| {
        (c.bare && k == c.name)
            || k == c.tail
            || (!project_folder.is_empty()
                && k.strip_suffix(c.tail.as_str())
                    .and_then(|rest| rest.strip_suffix('/'))
                    .is_some_and(|rest| rest.ends_with(&format!("/{project_folder}")) || rest == project_folder))
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
    let listed = listed(project_dir, relinks)?;
    relink_listed(raw, project_dir, &listed, &[]).map(|r| (r.bytes, r.changed))
}

/// A project pointed back at the Library.
struct Relinked {
    bytes: Vec<u8>,
    /// Which listed copies it no longer names where it was changed.
    changed: Vec<bool>,
    /// Files found again in a moved folder, said plainly.
    found: Vec<String>,
}

fn relink_listed(
    raw: &[u8],
    project_dir: &Path,
    listed: &[Listed],
    known_libraries: &[PathBuf],
) -> Result<Relinked, String> {
    let mut doc = bitwig::decode(raw).map_err(|_| "not a Bitwig project".to_owned())?;
    if let Some(e) = &doc.body_error {
        return Err(format!("the project couldn't be read to the end ({e}), so it's left as it is"));
    }
    let folder = key(&project_dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
    let mut changed = vec![false; listed.len()];
    // Points a text at the Library file, and says which copy it named.
    let point = |t: &mut Text| -> Option<usize> {
        let c = copy_of(t.as_str(), &folder, listed)?;
        let to = if t.as_str().contains(['/', '\\']) { &listed[c].full } else { &listed[c].library_name };
        *t = Text::new(to);
        Some(c)
    };
    // The metadata: the list of files the project uses lists only files inside the project's folder, so a copy leaves
    // it (as a Library file Bitwig didn't copy is never in it); a single text is pointed at the Library file.
    for (_, value) in &mut doc.meta {
        if let Value::Strings(ts) = value {
            ts.retain(|t| match copy_of(t.as_str(), &folder, listed) {
                Some(c) => {
                    changed[c] = true;
                    false
                }
                None => true,
            });
            continue;
        }
        for t in texts_mut(value) {
            if let Some(c) = point(t) {
                changed[c] = true;
            }
        }
    }
    // Each sample file that names a copy (where it is inside the project, its name, or any of its file references)
    // becomes the Library file, written as Bitwig writes a Library file it didn't copy: nothing inside the project's
    // folder (a path there is read from the project's folder, so the Library's would be missing), its file reference
    // and the files it was found at before all the Library file.
    let mut held: Vec<(ObjId, Vec<usize>)> = Vec::new();
    for i in 0..doc.objects.len() {
        let id = ObjId(i as u32);
        if doc.obj(id).class != SAMPLE_FILE {
            continue;
        }
        let refs = file_refs(&doc, id);
        let o = doc.obj(id);
        let texts = [F_IN_PROJECT, F_NAME].into_iter().filter_map(|f| match o.get(f) {
            Some(Value::Str(t)) => Some(t.as_str()),
            _ => None,
        });
        let paths = refs.iter().filter_map(|&r| match doc.obj(r).get(F_PATH) {
            Some(Value::Str(t)) => Some(t.as_str()),
            _ => None,
        });
        let Some(c) = texts.chain(paths).find_map(|t| copy_of(t, &folder, listed)) else { continue };
        let l = &listed[c];
        let o = doc.obj_mut(id);
        if o.has(F_IN_PROJECT) {
            o.set(F_IN_PROJECT, Value::Str(Text::new("")));
        }
        o.set(F_NAME, Value::Str(Text::new(&l.library_name)));
        for r in refs {
            doc.obj_mut(r).set(F_PATH, Value::Str(Text::new(&l.full)));
        }
        changed[c] = true;
        held.push((id, vec![c]));
    }
    // Any other text that names a copy (a layout not known here), and which copies each object held.
    for (i, o) in doc.objects.iter_mut().enumerate() {
        let mut here = Vec::new();
        for field in &mut o.fields {
            for t in texts_mut(&mut field.value) {
                if let Some(c) = point(t) {
                    changed[c] = true;
                    here.push(c);
                }
            }
        }
        if !here.is_empty() {
            held.push((ObjId(i as u32), here));
        }
    }
    // Beside each: where its copy came from (the old Library path, or the same copy in another project or template it
    // was copied from), if that's gone, names that copy's Library file now.
    let children = children(&doc);
    let mut on_disk: HashMap<String, bool> = HashMap::new();
    for (id, copies) in held {
        for near in around(&doc, &children, id) {
            for field in &mut doc.obj_mut(near).fields {
                let Value::Str(t) = &mut field.value else { continue };
                let s = t.as_str();
                let Some(c) = copies.iter().map(|&c| &listed[c]).find(|c| {
                    let name = key(file_name(s));
                    name == key(&c.library_name) || name == c.name
                }) else {
                    continue;
                };
                if !s.contains(['/', '\\']) || key(s) == key(&c.full) {
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
    // Bitwig's Library folders now: the ones given (Settings' first), then the ones the copies' Library files are in.
    let mut libraries: Vec<PathBuf> = Vec::new();
    for at in known_libraries {
        if at.is_dir() && !libraries.contains(at) {
            libraries.push(at.clone());
        }
    }
    for l in listed {
        let at = Path::new(&l.full)
            .ancestors()
            .find(|a| a.file_name().is_some_and(|n| key(&n.to_string_lossy()) == "LIBRARY"));
        if let Some(at) = at
            && !libraries.contains(&at.to_path_buf())
        {
            libraries.push(at.to_path_buf());
        }
    }
    let found = found_again(&mut doc, project_dir, &libraries);
    let bytes = bitwig::encode(&doc, None)?;
    Ok(Relinked { bytes, changed, found })
}

/// A sample file's file references: the one Bitwig finds it by, and the list of where it was found before.
fn file_refs(doc: &Document, file: ObjId) -> Vec<ObjId> {
    let mut out: Vec<ObjId> = doc.field_obj(file, F_FILE).into_iter().collect();
    if let Some(found) = doc.field_obj(file, F_FOUND) {
        out.extend(doc.field_list(found, F_FOUND_FILES));
    }
    out
}

/// A text's folders and name, split at either slash.
fn parts(path: &str) -> Vec<&str> {
    path.split(['/', '\\']).filter(|p| !p.is_empty()).collect()
}

/// Where the project's folder was when it was saved, if that's somewhere else now (Bitwig Studio's folder moved from
/// I: to K:, say): the old start of a path, the folder it went on with, and where that start is now. Learned from the
/// project's own texts: an absolute path through a folder of this project's name that ends the same way as where the
/// project is now. Most seen first.
fn moved_from(doc: &Document, project_dir: &Path) -> Vec<(Vec<String>, PathBuf)> {
    let here = std::path::absolute(project_dir).unwrap_or_else(|_| project_dir.to_path_buf());
    let here_text = here.to_string_lossy().into_owned();
    let now: Vec<String> = parts(&here_text).iter().map(|p| key(p)).collect();
    let Some(folder) = now.last() else { return Vec::new() };
    let mut seen: HashMap<Vec<String>, (usize, usize)> = HashMap::new();
    for o in &doc.objects {
        for f in &o.fields {
            let Value::Str(t) = &f.value else { continue };
            let s = t.as_str();
            if !paths::is_absolute(s) {
                continue;
            }
            let was: Vec<String> = parts(s).iter().map(|p| key(p)).collect();
            // The folder of this project's name, with something inside it.
            let Some(at) = (0..was.len().saturating_sub(1)).rev().find(|&i| &was[i] == folder) else { continue };
            let old_dir = &was[..=at];
            let same = old_dir.iter().rev().zip(now.iter().rev()).take_while(|(a, b)| a == b).count();
            if same == old_dir.len() || same == now.len() {
                // The same place, or one start is inside the other: nothing moved.
                continue;
            }
            // The old start, and the first folder both share: only paths through that folder are moved.
            let start = old_dir[..old_dir.len() - same + 1].to_vec();
            let entry = seen.entry(start).or_insert((0, same));
            entry.0 += 1;
        }
    }
    let mut out: Vec<(usize, Vec<String>, PathBuf)> = seen
        .into_iter()
        .filter_map(|(start, (count, same))| Some((count, start, here.ancestors().nth(same)?.to_path_buf())))
        .collect();
    out.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    out.into_iter().map(|(_, start, now)| (start, now)).collect()
}

/// An integer field's value, whatever size Bitwig saved it in.
fn integer(value: Option<&Value>) -> Option<u64> {
    match *value? {
        Value::U8(v) => Some(v.into()),
        Value::I16(v) => u64::try_from(v).ok(),
        Value::I32(v) => u64::try_from(v).ok(),
        Value::I64(v) => u64::try_from(v).ok(),
        _ => None,
    }
}

/// Sample files Bitwig can't find because the folder they were in has moved: with the project's (Bitwig Studio's
/// folder moved to another drive, its Library with it), or Bitwig's Library on its own (from Documents to K:, say).
/// Each file reference that's gone is pointed at the same place in the moved folder, or in Bitwig's Library now (what
/// came after "Library" in the old path), when a file is there with the same name and the size the project saved.
/// Files inside the project's folder that are there are found already and aren't touched. Says which files were found
/// again.
fn found_again(doc: &mut Document, project_dir: &Path, libraries: &[PathBuf]) -> Vec<String> {
    let moves = moved_from(doc, project_dir);
    let mut found = Vec::new();
    if moves.is_empty() && libraries.is_empty() {
        return found;
    }
    let files: Vec<ObjId> =
        (0..doc.objects.len()).map(|i| ObjId(i as u32)).filter(|&i| doc.obj(i).class == SAMPLE_FILE).collect();
    for file in files {
        let o = doc.obj(file);
        if let Some(Value::Str(inside)) = o.get(F_IN_PROJECT)
            && !inside.as_str().is_empty()
            && project_dir.join(inside.as_str()).is_file()
        {
            continue;
        }
        let size = integer(o.get(F_SIZE));
        // Each file reference: where it is now, if it's there (or found in a moved folder), and whether it moved.
        let mut gone = Vec::new();
        let mut there: Option<String> = None;
        let mut moved = false;
        for r in file_refs(doc, file) {
            let Some(Value::Str(t)) = doc.obj(r).get(F_PATH) else { continue };
            let old = t.as_str().to_owned();
            if !paths::is_absolute(&old) {
                continue;
            }
            if Path::new(&old).is_file() {
                there.get_or_insert(old);
                continue;
            }
            let was = parts(&old);
            let keys: Vec<String> = was.iter().map(|p| key(p)).collect();
            let same = |path: PathBuf| {
                let meta = std::fs::metadata(&path).ok()?;
                (meta.is_file() && size.is_none_or(|s| s == meta.len())).then_some(path)
            };
            let moved_with_project = moves.iter().find_map(|(start, now)| {
                if keys.len() <= start.len() || keys[..start.len()] != start[..] {
                    return None;
                }
                let mut path = now.clone();
                for p in &was[start.len() - 1..] {
                    path.push(p);
                }
                same(path)
            });
            let in_library = || {
                let at = keys[..keys.len() - 1].iter().rposition(|k| k == "LIBRARY")?;
                libraries.iter().find_map(|library| {
                    let mut path = library.clone();
                    for p in &was[at + 1..] {
                        path.push(p);
                    }
                    same(path)
                })
            };
            let new = moved_with_project.or_else(in_library);
            match new {
                Some(new) => {
                    let new = new.to_string_lossy().into_owned();
                    let said = format!("{} is now at {new}", file_name(&old));
                    if !found.contains(&said) {
                        found.push(said);
                    }
                    doc.obj_mut(r).set(F_PATH, Value::Str(Text::new(&new)));
                    there.get_or_insert(new);
                    moved = true;
                }
                None => gone.push(r),
            }
        }
        // One that's still gone (the copy in another project it came from, say) names the file where it is, so Bitwig
        // finds it whichever reference it reads first.
        if let Some(there) = there.filter(|_| moved) {
            for r in gone {
                doc.obj_mut(r).set(F_PATH, Value::Str(Text::new(&there)));
            }
        }
    }
    found
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

/// Where a Bitwig file's zip starts, from its header (the last number in it), if it has one.
fn zip_start(raw: &[u8]) -> Option<usize> {
    let hex = raw.get(24..40).and_then(|h| std::str::from_utf8(h).ok())?;
    let at = usize::from_str_radix(hex, 16).ok()?;
    (at > 0).then_some(at)
}

/// Which copies a Bitwig file's bytes may still name anywhere: its texts (in either text form Bitwig writes), plugin
/// settings, and the zip at the end, unpacked. Only ASCII letters are compared without case, and anything that can't
/// be looked through counts as naming every copy, so this errs towards "still named".
fn named_in(raw: &[u8], listed: &[Listed]) -> Vec<bool> {
    let mut named = vec![false; listed.len()];
    let mut look = |bytes: &[u8]| {
        let lower = bytes.to_ascii_lowercase();
        for (c, copy) in listed.iter().enumerate() {
            named[c] = named[c] || copy.needles.iter().any(|n| memmem::find(&lower, n).is_some());
        }
    };
    if !bitwig::is_bitwig(raw) {
        return vec![true; listed.len()];
    }
    look(raw);
    let Some(start) = zip_start(raw) else { return named };
    let unpacked = (|| {
        let mut archive = zip::ZipArchive::new(Cursor::new(raw.get(start..)?)).ok()?;
        for i in 0..archive.len() {
            let mut entry = Vec::new();
            archive.by_index(i).ok()?.take(LONGEST_ZIP_ENTRY + 1).read_to_end(&mut entry).ok()?;
            if entry.len() as u64 > LONGEST_ZIP_ENTRY {
                return None;
            }
            look(&entry);
        }
        Some(())
    })();
    if unpacked.is_none() {
        return vec![true; listed.len()];
    }
    named
}

/// Whether two files hold the same bytes, read a piece at a time.
fn same_file(a: &Path, b: &Path) -> bool {
    let (Ok(fa), Ok(fb)) = (std::fs::File::open(a), std::fs::File::open(b)) else { return false };
    let same_size = match (fa.metadata(), fb.metadata()) {
        (Ok(ma), Ok(mb)) => ma.len() == mb.len(),
        _ => false,
    };
    if !same_size {
        return false;
    }
    let (mut ra, mut rb) = (std::io::BufReader::new(fa), std::io::BufReader::new(fb));
    let (mut ba, mut bb) = (vec![0u8; 1 << 16], vec![0u8; 1 << 16]);
    loop {
        let Ok(n) = ra.read(&mut ba) else { return false };
        if n == 0 {
            return rb.read(&mut bb[..1]).is_ok_and(|m| m == 0);
        }
        if rb.read_exact(&mut bb[..n]).is_err() || ba[..n] != bb[..n] {
            return false;
        }
    }
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
    fix_bitwig_project_in(project, relinks, &[])
}

/// [`fix_bitwig_project`], with Bitwig's Library folders as you have them (your Bitwig Studio folder's in Settings
/// first): a file whose folder moved is looked for in these first, then in the Library the copies' files are in.
pub fn fix_bitwig_project_in(project: &Path, relinks: &[Relink], libraries: &[PathBuf]) -> Result<Fixed, String> {
    let dir = match project.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let listed = listed(&dir, relinks)?;
    // A copy that's no longer the same as its Library file (edited in Bitwig since) is left alone.
    let same: Vec<bool> = relinks.par_iter().map(|r| same_file(&r.copy, &r.library)).collect();
    let mut left = Vec::new();
    let mut keep = Vec::new();
    let mut kept_relinks = Vec::new();
    for ((r, l), same) in relinks.iter().zip(listed).zip(same) {
        if same {
            keep.push(l);
            kept_relinks.push(r.clone());
        } else {
            left.push(format!("{}: it isn't the same as its Library file (any more), so it's left alone", l.shown));
        }
    }
    let (listed, relinks) = (keep, kept_relinks);
    if listed.is_empty() {
        return Err(format!("no copy is the same as its Library file, so nothing was written: {}", left.join("; ")));
    }
    let raw = std::fs::read(project).map_err(|e| format!("Couldn't read {}: {e}", project.display()))?;
    let Relinked { bytes, mut changed, found } = relink_listed(&raw, &dir, &listed, libraries)?;
    if !changed.contains(&true) {
        left.push("the project uses none of the other copies".to_owned());
        return Err(format!("nothing was written: {}", left.join("; ")));
    }
    // Still named in the fixed project somewhere this doesn't change (a plugin's own settings).
    for (c, still) in named_in(&bytes, &listed).into_iter().enumerate() {
        if still {
            let was = if changed[c] {
                "still names it somewhere that isn't changed"
            } else {
                "names it in a way that isn't changed"
            };
            left.push(format!("{}: the project {was}, so it's kept", listed[c].shown));
            changed[c] = false;
        } else if !changed[c] {
            left.push(format!("{}: this project doesn't use it, so it's left alone", listed[c].shown));
        }
    }
    if !changed.contains(&true) {
        return Err(format!("every copy is still needed, so nothing was written: {}", left.join("; ")));
    }
    // Other Bitwig files in the folder and below it may use the same copies.
    let others = other_bitwig_files(project, &dir)?;
    let used: Vec<(String, Vec<bool>)> = others
        .par_iter()
        .map(|p| {
            let shown = p.strip_prefix(&dir).unwrap_or(p).to_string_lossy().into_owned();
            match std::fs::read(p) {
                // An empty file names nothing (a save that was cut short).
                Ok(raw) if raw.is_empty() => (shown, vec![false; listed.len()]),
                Ok(raw) => (shown, named_in(&raw, &listed)),
                // One that can't be read might use any of them.
                Err(_) => (shown, vec![true; listed.len()]),
            }
        })
        .collect();
    for (c, l) in listed.iter().enumerate() {
        if let Some((other, _)) = used.iter().find(|(_, u)| changed[c] && u[c]) {
            changed[c] = false;
            left.push(format!("{}: {other} in the project's folder may still use it, so it's kept", l.shown));
        }
    }

    // The fixed project must read back cleanly (save_checked_new has made sure it holds these bytes, which are
    // already looked through above).
    let problem = |back: &[u8]| match bitwig::decode(back) {
        Ok(doc) => doc.body_error,
        Err(_) => Some("not a Bitwig project".to_owned()),
    };
    let mut file = None;
    for name in fixed_names(project) {
        if save_checked_new(&name, &bytes, problem)? {
            file = Some(name);
            break;
        }
    }
    let file = file.ok_or("every \"(fixed …)\" name up to 999 is taken, so nothing was written")?;
    let changed = relinks.iter().zip(&changed).filter(|(_, c)| **c).map(|(r, _)| r.clone()).collect();
    Ok(Fixed { file, changed, left, found })
}

/// Asked again just before a copy goes to the Recycle Bin (Solomon may have saved new versions since the fixed project
/// was written): why `relink.copy` is still needed, in plain words, or None when nothing but `original` names it. Every
/// Bitwig file in the project's folder and below it is looked through (the fixed projects and auto-backups too); one
/// that can't be read, a folder that can't be listed, or a Library file that's gone keeps the copy.
pub fn still_named(original: &Path, relink: &Relink) -> Option<String> {
    let dir = match original.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let shown = relink.copy.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let listed = match listed(&dir, std::slice::from_ref(relink)) {
        Ok(l) => l,
        Err(e) => return Some(format!("{shown} stays: {e}.")),
    };
    let others = match other_bitwig_files(original, &dir) {
        Ok(o) => o,
        Err(e) => return Some(format!("{shown} stays: {e}.")),
    };
    others.par_iter().find_map_first(|p| {
        let named = match std::fs::read(p) {
            Ok(raw) if raw.is_empty() => false,
            Ok(raw) => named_in(&raw, &listed)[0],
            Err(_) => true,
        };
        named.then(|| {
            let other = p.strip_prefix(&dir).unwrap_or(p).to_string_lossy().into_owned();
            format!("{other} still uses {shown}, so it stays.")
        })
    })
}

/// The other Bitwig files in the project's folder and below it (Bitwig's auto-backups, an earlier fixed project, clips
/// and presets saved there), leaving out the project itself and a Mac's "._" shadow files. A folder that can't be
/// listed is said, since a copy can't be offered for the bin without looking.
fn other_bitwig_files(project: &Path, dir: &Path) -> Result<Vec<PathBuf>, String> {
    let itself = key(&std::path::absolute(project).unwrap_or_else(|_| project.to_path_buf()).to_string_lossy());
    let mut out = Vec::new();
    let mut folders = vec![dir.to_path_buf()];
    while let Some(folder) = folders.pop() {
        let listing = |e: std::io::Error| format!("Couldn't look through {}: {e}", folder.display());
        for entry in std::fs::read_dir(&folder).map_err(listing)? {
            let entry = entry.map_err(listing)?;
            let path = entry.path();
            let kind = entry.file_type().map_err(listing)?;
            if kind.is_dir() {
                folders.push(path);
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let bitwig = path.extension().is_some_and(|x| BITWIG_FILES.iter().any(|b| x.eq_ignore_ascii_case(b)));
            let full = std::path::absolute(&path).unwrap_or_else(|_| path.clone());
            if bitwig && !name.starts_with("._") && key(&full.to_string_lossy()) != itself {
                out.push(path);
            }
        }
    }
    Ok(out)
}
