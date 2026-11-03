//! The second backup format (Solomon, 31 Oct 2026): one zip per song, holding the song's saves the level keeps and
//! every sample they use, in `<backup>\Songs\<song>.zip`. Tidy and easy to move or send; saves shrink a lot, sounds
//! barely (they're stored as they are, which also keeps packing quick).
//!
//! Inside, each file keeps its whole path under its drive letter (`I/Fl Studio Data/…/Song.flp`), as the folder
//! backup lays them out, so two samples with the same name never clash, and each keeps its date. A `pack.txt` inside
//! says which song the zip belongs to and lists what went in (size, time, place), so a song whose files haven't
//! changed isn't packed again, and two songs can never take turns writing over one zip.
//!
//! A zip is written under a temporary name, then every entry is read back (the zip's own check of each file's bytes),
//! and only then does it take its real name. Your music folders are only ever read.
//!
//! A zip holds one version of each file it lists: a save you break by mistake replaces the good one in the zip on the
//! next run. The song's milestones (the last save of each day, big changes, saves you mark) are separate saves, so
//! they stay in it; the folder backup's "Older versions" is the format for keeping every replaced file.
//!
//! A file the zip holds that's gone from your folders (deleted, moved, or on a drive that isn't plugged in) stays in
//! it when the song is packed again (Solomon, 3 Nov 2026: deleted from your music isn't deleted from the backup;
//! [`crate::gone`] finds them for the page's warning). One that's still in your folders but no longer the song's (a
//! sample swapped for another, an autosave the level leaves out) goes from the zip: you still have it.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{Datelike, Local, TimeZone, Timelike};
use mo_formats::textscan::{safe_file_name, unsafe_in_file_name};
use mo_space::size::is_sound;
use rayon::prelude::*;
use zip::result::ZipError;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipArchive, ZipWriter};

use crate::copy::{BUFFER, CantStart, Drive, FAT32_LARGEST, PART, put_in_place, read_some, remove_part};
use crate::plan::{from_backup, place_on_backup};
use crate::record::nanos;

/// The folder on the backup that holds the song zips.
pub const SONGS: &str = "Songs";
/// The list of what's inside, kept inside each zip.
const LIST: &str = "pack.txt";
/// From this size an entry takes the zip's large-file form. Under 4 GiB by a margin, since a file that doesn't
/// compress grows a little when deflated.
const LARGE: u64 = 0xFFFF_FFFF - (0xFFFF_FFFF >> 4);
/// The longest song title a zip name keeps, so `<name> (12).zip.mo-part` stays under every system's 255.
const LONGEST_TITLE: usize = 200;

/// What happened to one song's zip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Packed {
    /// Made (or made again because something in it changed).
    Written {
        bytes: u64,
        /// The files it kept from the zip before that aren't in your folders now.
        kept: usize,
        /// Files it would have kept from the zip before but couldn't, because they were damaged in it (their places
        /// inside the zip).
        damaged: Vec<String>,
    },
    /// Nothing in it changed since it was made.
    UpToDate {
        /// The files it keeps that aren't in your folders now.
        kept: usize,
    },
}

/// A zip name for each song, in the order given: its title made safe for every system (and cut short), and a
/// number for a song whose name another song already has, letter case aside. Songs are named in the order of their
/// ids, so a song keeps its name when a newer song of the same title comes along.
pub fn pack_names(songs: &[(&str, u64)]) -> Vec<String> {
    let mut order: Vec<usize> = (0..songs.len()).collect();
    order.sort_by_key(|&i| songs[i].1);
    let mut taken: HashSet<String> = HashSet::new();
    let mut names = vec![String::new(); songs.len()];
    for i in order {
        let base: String = safe_file_name(songs[i].0).chars().take(LONGEST_TITLE).collect();
        let base = base.trim_end_matches([' ', '.']).to_owned();
        let mut name = format!("{base}.zip");
        let mut n = 2;
        while !taken.insert(name.to_lowercase()) {
            name = format!("{base} ({n}).zip");
            n += 1;
        }
        names[i] = name;
    }
    names
}

/// Whether a zip name is one plain, safe file name (as [`pack_names`] makes them).
fn good_name(name: &str) -> bool {
    let mut parts = Path::new(name).components();
    matches!((parts.next(), parts.next()), (Some(Component::Normal(_)), None))
        && !name.chars().any(unsafe_in_file_name)
        && name.ends_with(".zip")
        && name.chars().count() <= LONGEST_TITLE + 20
}

/// Where a file goes inside a zip, or `None` for a path that isn't whole.
fn inside(path: &Path) -> Option<String> {
    let rel = place_on_backup(Path::new(""), path)?;
    let parts: Vec<String> = rel.iter().map(|p| p.to_string_lossy().into_owned()).collect();
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// An error that says which file it was about.
fn about(path: &Path, e: io::Error) -> io::Error {
    io::Error::new(e.kind(), format!("{}: {e}", path.display()))
}

/// Where a file in a song's zip comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Source {
    /// Your folders.
    File(PathBuf),
    /// The zip that was there before: a file gone from your folders, kept.
    Kept,
}

/// One file in a song's zip, as it was when listed.
struct Item {
    source: Source,
    inside: String,
    size: u64,
    time: i128,
}

/// Packs songs onto one backup drive, noticing the drive going away between songs (or while one is packed).
pub struct Packer<'a> {
    backup: &'a Path,
    drive: Drive<'a>,
    ignores_case: bool,
}

impl<'a> Packer<'a> {
    /// Ready to pack onto `backup`, which must already be there (nothing is created in its place).
    pub fn new(backup: &'a Path) -> Result<Packer<'a>, CantStart> {
        let drive = Drive::find(backup).ok_or_else(|| CantStart::NoBackupFolder(backup.to_path_buf()))?;
        Ok(Packer { backup, drive, ignores_case: cfg!(any(windows, target_os = "macos")) })
    }

    fn drive_there(&self) -> io::Result<()> {
        if self.drive.still_there() {
            Ok(())
        } else {
            Err(io::Error::new(io::ErrorKind::NotFound, "The backup drive isn't there any more"))
        }
    }

    /// Packs one song's files into `<backup>\Songs\<name>` (a name from [`pack_names`]; `song` is the song's id),
    /// unless the zip there already holds them as they are now. A file the zip there holds that isn't among them stays
    /// in it while it isn't in your folders (deleted, moved, or on a drive that isn't plugged in). A file that can't be
    /// read or placed fails the whole zip (a song pack missing a sample would look whole and not be), and so does a zip
    /// of that name that belongs to another song; the zip already there stays as it was.
    pub fn pack_song(&self, files: &[PathBuf], name: &str, song: u64, stop: &AtomicBool) -> io::Result<Packed> {
        if !good_name(name) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("\"{name}\" isn't a zip name")));
        }
        if files.is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "No files to pack: the zip there is kept"));
        }
        self.drive_there()?;
        let mut items: Vec<Item> = Vec::with_capacity(files.len());
        let mut seen = HashSet::new();
        for f in files {
            let meta = fs::metadata(f).map_err(|e| about(f, e))?;
            if !meta.is_file() {
                return Err(about(f, io::Error::other("not a file")));
            }
            let at = inside(f).ok_or_else(|| about(f, io::Error::other("can't be placed in a zip")))?;
            if seen.insert(self.fold(&at)) {
                let time = nanos(meta.modified().map_err(|e| about(f, e))?);
                items.push(Item { source: Source::File(f.clone()), inside: at, size: meta.len(), time });
            }
        }

        let folder = self.backup.join(SONGS);
        let dest = folder.join(name);
        let old = if fs::metadata(&dest).is_ok_and(|m| m.is_file()) { read_old(&dest, song)? } else { None };
        // What the zip before holds that isn't in the list now stays, while it isn't in your folders (the system is
        // only asked about those). One it can't tell about (a drive that isn't plugged in) stays too.
        let mut kept = 0;
        for (size, time, inside) in old.iter().flat_map(|o| &o.lines) {
            let key = self.fold(inside);
            if seen.contains(&key) {
                continue;
            }
            let there = from_backup(inside).is_some_and(|p| fs::metadata(p).is_ok_and(|m| m.is_file()));
            if !there {
                seen.insert(key);
                items.push(Item { source: Source::Kept, inside: inside.clone(), size: *size, time: *time });
                kept += 1;
            }
        }
        items.sort_by(|a, b| a.inside.cmp(&b.inside));
        if let Some(old) = &old
            && same(old, &dest, &list_text(song, items.iter()), &items, stop)?
        {
            return Ok(Packed::UpToDate { kept });
        }
        fs::create_dir_all(&folder)?;
        let part = folder.join(format!("{name}{PART}"));
        match fs::symlink_metadata(&part) {
            Ok(m) if m.is_file() => remove_part(&part),
            Ok(_) => return Err(io::Error::other("Something that isn't a file is in the way of the zip")),
            Err(_) => {}
        }
        let total: u64 = items.iter().map(|i| i.size).sum();
        let written = write_zip(&items, song, &dest, &part, stop)
            .and_then(|damaged| check_zip(&part, &items, &damaged, stop).map(|bytes| (bytes, damaged)))
            .and_then(|done| self.drive_there().map(|()| done));
        match written.and_then(|done| put_in_place(&part, &dest).map(|()| done)) {
            Ok((bytes, damaged)) => Ok(Packed::Written { bytes, kept: kept.saturating_sub(damaged.len()), damaged }),
            Err(e) => {
                remove_part(&part);
                if e.kind() == io::ErrorKind::StorageFull && total > FAT32_LARGEST {
                    // On a FAT32 drive a file can't pass 4 GB, and the system calls that a full drive.
                    return Err(io::Error::other(
                        "This song is over 4 GB, more than one file can hold on a FAT32 drive (or the drive is full)",
                    ));
                }
                Err(e)
            }
        }
    }
}

impl Packer<'_> {
    /// A place inside a zip as it's compared: small letters on a system that ignores letter case.
    fn fold(&self, inside: &str) -> String {
        if self.ignores_case { inside.to_lowercase() } else { inside.to_owned() }
    }
}

fn list_text<'a>(song: u64, items: impl Iterator<Item = &'a Item>) -> String {
    let mut text = format!("song\t{song}\n");
    for i in items {
        text.push_str(&format!("{}\t{}\t{}\n", i.size, i.time, i.inside));
    }
    text
}

/// One line of a zip's list: a file's size, its time, and its place inside the zip.
type Line = (u64, i128, String);

/// The list in the zip already on the backup.
struct Old {
    /// As written.
    text: String,
    lines: Vec<Line>,
}

/// The song id and list in a zip, or `None` when it can't be read (or isn't one of the app's).
fn open_list(zip: &Path) -> Option<(u64, String)> {
    let mut archive = ZipArchive::new(File::open(zip).ok()?).ok()?;
    let mut text = String::new();
    archive.by_name(LIST).ok()?.read_to_string(&mut text).ok()?;
    let song = text.lines().next()?.strip_prefix("song\t")?.parse().ok()?;
    Some((song, text))
}

/// A file's line in a list; `None` for one that doesn't make sense.
fn parse_line(line: &str) -> Option<Line> {
    let mut p = line.splitn(3, '\t');
    let size = p.next()?.parse().ok()?;
    let time = p.next()?.parse().ok()?;
    let inside = p.next()?;
    inside_ok(inside).then(|| (size, time, inside.to_owned()))
}

/// Whether a place inside a zip is plain names under a drive's folder, as [`inside`] makes them.
fn inside_ok(inside: &str) -> bool {
    inside != LIST
        && inside.split('/').all(|part| {
            let mut c = Path::new(part).components();
            matches!((c.next(), c.next()), (Some(Component::Normal(_)), None))
        })
}

/// The list of the zip already at `zip`. `None` when it can't be read: the song is packed again, whole. An error when
/// it's another song's.
fn read_old(zip: &Path, song: u64) -> io::Result<Option<Old>> {
    let Some((owner, text)) = open_list(zip) else { return Ok(None) };
    if owner != song {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("{} belongs to another song", zip.display())));
    }
    let lines = text.lines().skip(1).filter_map(parse_line).collect();
    Ok(Some(Old { text, lines }))
}

/// Whether the zip already there holds the files as they are now: the same list, or the same files at the same sizes
/// with only some times moved (a FAT or exFAT drive after the clocks change, a copy that took new times), whose bytes
/// match the zip's own check numbers.
fn same(old: &Old, zip: &Path, list: &str, items: &[Item], stop: &AtomicBool) -> io::Result<bool> {
    if old.text == list {
        return Ok(true);
    }
    let old_items: HashMap<&str, u64> = old.lines.iter().map(|(size, _, inside)| (inside.as_str(), *size)).collect();
    if old_items.len() != items.len() || items.iter().any(|i| old_items.get(i.inside.as_str()) != Some(&i.size)) {
        return Ok(false);
    }
    let Ok(file) = File::open(zip) else { return Ok(false) };
    let Ok(mut archive) = ZipArchive::new(file) else { return Ok(false) };
    let mut buf = vec![0u8; BUFFER];
    for item in items {
        // A kept file's line is the zip's own.
        let Source::File(source) = &item.source else { continue };
        let Ok(entry) = archive.by_name(&item.inside) else { return Ok(false) };
        let crc = entry.crc32();
        drop(entry);
        let mut src = File::open(source).map_err(|e| about(source, e))?;
        let mut hasher = crc32fast::Hasher::new();
        while let Some(n) = read_some(&mut src, &mut buf, stop).map_err(|e| about(source, e))? {
            hasher.update(&buf[..n]);
        }
        if hasher.finalize() != crc {
            return Ok(false);
        }
    }
    Ok(true)
}

/// One song zip on the backup: the zip, and each file its list names.
#[derive(Clone, Debug)]
pub struct ZipList {
    pub zip: PathBuf,
    /// Each file's size, its time when packed (nanoseconds since 1970) and its place inside the zip.
    pub files: Vec<(u64, i128, String)>,
}

/// The lists of every song zip on the backup, several read at a time on the current rayon pool. A zip that can't be
/// read is left out (what's in it can't be told); no Songs folder, none.
pub fn lists(backup: &Path, stop: &AtomicBool) -> io::Result<Vec<ZipList>> {
    let entries = match fs::read_dir(backup.join(SONGS)) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let zips: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        // Not through links: only the zips really in the Songs folder.
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("zip")))
        .collect();
    let lists: Vec<ZipList> = zips
        .into_par_iter()
        .filter_map(|zip| {
            if stop.load(Ordering::Relaxed) {
                return None;
            }
            let (_, text) = open_list(&zip)?;
            let files = text.lines().skip(1).filter_map(parse_line).collect();
            Some(ZipList { zip, files })
        })
        .collect();
    if stop.load(Ordering::Relaxed) {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "Stopped"));
    }
    Ok(lists)
}

/// A file's time as a zip keeps it (local time, to two seconds), or none for a time a zip can't hold.
fn zip_time(time: i128) -> Option<DateTime> {
    let secs = i64::try_from(time.div_euclid(1_000_000_000)).ok()?;
    let local = Local.timestamp_opt(secs, 0).earliest()?;
    DateTime::from_date_and_time(
        u16::try_from(local.year()).ok()?,
        u8::try_from(local.month()).ok()?,
        u8::try_from(local.day()).ok()?,
        u8::try_from(local.hour()).ok()?,
        u8::try_from(local.minute()).ok()?,
        u8::try_from(local.second()).ok()?,
    )
    .ok()
}

/// Writes the zip under its temporary name: each file from your folders, each kept one from the zip before (`old`),
/// then the list. A kept file that's damaged in the zip before (its bytes don't match its check number, say) is left
/// out, and its place inside is given back; anything else that goes wrong fails the zip.
fn write_zip(items: &[Item], song: u64, old: &Path, part: &Path, stop: &AtomicBool) -> io::Result<Vec<String>> {
    let file = File::create_new(part)?;
    let mut zip = ZipWriter::new(io::BufWriter::new(file));
    let mut buf = vec![0u8; BUFFER];
    // The zip before, when a file is kept from it.
    let mut before = match items.iter().any(|i| i.source == Source::Kept) {
        true => Some(ZipArchive::new(File::open(old)?)?),
        false => None,
    };
    let mut damaged: Vec<String> = Vec::new();
    for item in items {
        // Sounds (audio, sample-library files, anything big) barely shrink: stored as they are. Projects, presets and
        // MIDI shrink a lot.
        let method =
            if is_sound(&item.inside, item.size) { CompressionMethod::Stored } else { CompressionMethod::Deflated };
        let mut options = SimpleFileOptions::default().compression_method(method).large_file(item.size >= LARGE);
        if let Some(when) = zip_time(item.time) {
            options = options.last_modified_time(when);
        }
        zip.start_file(item.inside.as_str(), options)?;
        match &item.source {
            Source::File(source) => copy_file(source, item, &mut zip, &mut buf, stop)?,
            Source::Kept => {
                let Some(archive) = before.as_mut() else {
                    return Err(io::Error::other("The zip before couldn't be opened"));
                };
                match copy_kept(archive, item, &mut zip, &mut buf, stop) {
                    Ok(()) => {}
                    Err(e) if is_damage(&e) => {
                        zip.abort_file()?;
                        damaged.push(item.inside.clone());
                    }
                    Err(e) => return Err(e),
                }
            }
        }
    }
    zip.start_file(LIST, SimpleFileOptions::default())?;
    zip.write_all(list_text(song, items.iter().filter(|i| !damaged.contains(&i.inside))).as_bytes())?;
    let out = zip.finish()?;
    out.into_inner().map_err(io::IntoInnerError::into_error)?.sync_all()?;
    Ok(damaged)
}

/// Copies a file from your folders into the zip, and checks it didn't change while it was read.
fn copy_file(
    source: &Path,
    item: &Item,
    zip: &mut ZipWriter<io::BufWriter<File>>,
    buf: &mut [u8],
    stop: &AtomicBool,
) -> io::Result<()> {
    let mut src = File::open(source).map_err(|e| about(source, e))?;
    let mut total: u64 = 0;
    while let Some(n) = read_some(&mut src, buf, stop).map_err(|e| about(source, e))? {
        zip.write_all(&buf[..n])?;
        total = total.saturating_add(n as u64);
    }
    let after = fs::metadata(source).map_err(|e| about(source, e))?;
    if total != item.size || after.len() != item.size || after.modified().map(nanos).ok() != Some(item.time) {
        return Err(io::Error::other(format!(
            "{} changed while it was being packed (saved again?): it goes next time",
            source.display()
        )));
    }
    Ok(())
}

/// Copies a kept file out of the zip before into the new one, reading it to the end so its bytes are checked against
/// its check number on the way. Damaged there, or not in it after all: an error [`is_damage`] knows.
fn copy_kept(
    archive: &mut ZipArchive<File>,
    item: &Item,
    zip: &mut ZipWriter<io::BufWriter<File>>,
    buf: &mut [u8],
    stop: &AtomicBool,
) -> io::Result<()> {
    let damage = |why: String| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {why}", item.inside));
    let mut entry = match archive.by_name(&item.inside) {
        Ok(entry) => entry,
        Err(ZipError::Io(e)) => return Err(e),
        Err(e) => return Err(damage(e.to_string())),
    };
    let mut total: u64 = 0;
    loop {
        if stop.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "Stopped"));
        }
        let n = match entry.read(buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        zip.write_all(&buf[..n])?;
        total = total.saturating_add(n as u64);
    }
    if total != item.size {
        return Err(damage("not the size its list says".to_owned()));
    }
    Ok(())
}

/// Whether reading a kept file failed because it's damaged in the zip before (rather than the drive failing).
fn is_damage(e: &io::Error) -> bool {
    matches!(e.kind(), io::ErrorKind::InvalidData | io::ErrorKind::InvalidInput | io::ErrorKind::UnexpectedEof)
}

/// Reads every entry of the new zip back to the end (the zip checks each one's bytes against the check number it
/// stored) and checks each is the size it should be. Gives the zip's size.
fn check_zip(part: &Path, items: &[Item], damaged: &[String], stop: &AtomicBool) -> io::Result<u64> {
    let mut archive = ZipArchive::new(File::open(part)?)?;
    let items: Vec<&Item> = items.iter().filter(|i| !damaged.contains(&i.inside)).collect();
    if archive.len() != items.len() + 1 {
        return Err(io::Error::other("The zip read back with the wrong number of files"));
    }
    let mut buf = vec![0u8; BUFFER];
    for item in items {
        let mut entry = archive.by_name(&item.inside)?;
        let mut total: u64 = 0;
        loop {
            if stop.load(Ordering::Relaxed) {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "Stopped"));
            }
            let n = match entry.read(&mut buf) {
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    return Err(io::Error::other(format!(
                        "{} read back wrongly ({e}): the backup drive may be failing",
                        item.inside
                    )));
                }
            };
            if n == 0 {
                break;
            }
            total = total.saturating_add(n as u64);
        }
        if total != item.size {
            return Err(io::Error::other(format!("{} read back at the wrong size", item.inside)));
        }
    }
    fs::metadata(part).map(|m| m.len())
}
