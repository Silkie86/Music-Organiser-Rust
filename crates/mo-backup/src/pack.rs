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

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::AtomicBool;

use chrono::{Datelike, Local, TimeZone, Timelike};
use mo_formats::textscan::{safe_file_name, unsafe_in_file_name};
use mo_space::size::is_sound;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipArchive, ZipWriter};

use crate::copy::{BUFFER, CantStart, Drive, FAT32_LARGEST, PART, put_in_place, read_some, remove_part};
use crate::plan::place_on_backup;
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
    Written { bytes: u64 },
    /// Nothing in it changed since it was made.
    UpToDate,
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

/// One file in a song's zip, as it was when listed.
struct Item {
    source: PathBuf,
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
    /// unless the zip there already holds them as they are now. A file that can't be read or placed fails the whole
    /// zip (a song pack missing a sample would look whole and not be), and so does a zip of that name that belongs to
    /// another song; the zip already there stays as it was.
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
            let key = if self.ignores_case { at.to_lowercase() } else { at.clone() };
            if seen.insert(key) {
                let time = nanos(meta.modified().map_err(|e| about(f, e))?);
                items.push(Item { source: f.clone(), inside: at, size: meta.len(), time });
            }
        }
        items.sort_by(|a, b| a.inside.cmp(&b.inside));
        let list = list_text(song, &items);

        let folder = self.backup.join(SONGS);
        let dest = folder.join(name);
        if fs::metadata(&dest).is_ok_and(|m| m.is_file()) {
            match up_to_date(&dest, song, &list, &items, stop)? {
                Old::Same => return Ok(Packed::UpToDate),
                Old::Other => {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        format!("{} belongs to another song", dest.display()),
                    ));
                }
                Old::Changed => {}
            }
        }
        fs::create_dir_all(&folder)?;
        let part = folder.join(format!("{name}{PART}"));
        match fs::symlink_metadata(&part) {
            Ok(m) if m.is_file() => remove_part(&part),
            Ok(_) => return Err(io::Error::other("Something that isn't a file is in the way of the zip")),
            Err(_) => {}
        }
        let total: u64 = items.iter().map(|i| i.size).sum();
        let written = write_zip(&items, &list, &part, stop)
            .and_then(|()| check_zip(&part, &items, stop))
            .and_then(|bytes| self.drive_there().map(|()| bytes));
        match written.and_then(|bytes| put_in_place(&part, &dest).map(|()| bytes)) {
            Ok(bytes) => Ok(Packed::Written { bytes }),
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

fn list_text(song: u64, items: &[Item]) -> String {
    let mut text = format!("song\t{song}\n");
    for i in items {
        text.push_str(&format!("{}\t{}\t{}\n", i.size, i.time, i.inside));
    }
    text
}

/// What the zip already on the backup holds.
enum Old {
    /// The files as they are now.
    Same,
    /// Another song's files.
    Other,
    /// Something changed (or it can't be read): pack again.
    Changed,
}

fn up_to_date(zip: &Path, song: u64, list: &str, items: &[Item], stop: &AtomicBool) -> io::Result<Old> {
    let Ok(file) = File::open(zip) else { return Ok(Old::Changed) };
    let Ok(mut archive) = ZipArchive::new(file) else { return Ok(Old::Changed) };
    let mut old = String::new();
    match archive.by_name(LIST) {
        Ok(mut entry) => {
            if entry.read_to_string(&mut old).is_err() {
                return Ok(Old::Changed);
            }
        }
        Err(_) => return Ok(Old::Changed),
    }
    let mut lines = old.lines();
    if lines.next() != Some(format!("song\t{song}").as_str()) {
        return Ok(Old::Other);
    }
    if old == list {
        return Ok(Old::Same);
    }
    // The same files at the same sizes, only some times moved (a FAT or exFAT drive after the clocks change, a copy
    // that took new times): compare those files' bytes with the zip's own check numbers before packing them all again.
    let old_items: HashMap<&str, u64> = lines
        .filter_map(|l| {
            let mut p = l.splitn(3, '\t');
            let size = p.next()?.parse().ok()?;
            p.next()?;
            Some((p.next()?, size))
        })
        .collect();
    if old_items.len() != items.len() || items.iter().any(|i| old_items.get(i.inside.as_str()) != Some(&i.size)) {
        return Ok(Old::Changed);
    }
    let mut buf = vec![0u8; BUFFER];
    for item in items {
        let Ok(entry) = archive.by_name(&item.inside) else { return Ok(Old::Changed) };
        let crc = entry.crc32();
        drop(entry);
        let mut src = File::open(&item.source).map_err(|e| about(&item.source, e))?;
        let mut hasher = crc32fast::Hasher::new();
        while let Some(n) = read_some(&mut src, &mut buf, stop).map_err(|e| about(&item.source, e))? {
            hasher.update(&buf[..n]);
        }
        if hasher.finalize() != crc {
            return Ok(Old::Changed);
        }
    }
    Ok(Old::Same)
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

fn write_zip(items: &[Item], list: &str, part: &Path, stop: &AtomicBool) -> io::Result<()> {
    let file = File::create_new(part)?;
    let mut zip = ZipWriter::new(io::BufWriter::new(file));
    let mut buf = vec![0u8; BUFFER];
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
        let mut src = File::open(&item.source).map_err(|e| about(&item.source, e))?;
        let mut total: u64 = 0;
        while let Some(n) = read_some(&mut src, &mut buf, stop).map_err(|e| about(&item.source, e))? {
            zip.write_all(&buf[..n])?;
            total = total.saturating_add(n as u64);
        }
        let after = fs::metadata(&item.source).map_err(|e| about(&item.source, e))?;
        if total != item.size || after.len() != item.size || after.modified().map(nanos).ok() != Some(item.time) {
            return Err(io::Error::other(format!(
                "{} changed while it was being packed (saved again?): it goes next time",
                item.source.display()
            )));
        }
    }
    zip.start_file(LIST, SimpleFileOptions::default())?;
    zip.write_all(list.as_bytes())?;
    let out = zip.finish()?;
    out.into_inner().map_err(io::IntoInnerError::into_error)?.sync_all()
}

/// Reads every entry of the new zip back to the end (the zip checks each one's bytes against the check number it
/// stored) and checks each is the size it should be. Gives the zip's size.
fn check_zip(part: &Path, items: &[Item], stop: &AtomicBool) -> io::Result<u64> {
    let mut archive = ZipArchive::new(File::open(part)?)?;
    if archive.len() != items.len() + 1 {
        return Err(io::Error::other("The zip read back with the wrong number of files"));
    }
    let mut buf = vec![0u8; BUFFER];
    for item in items {
        let mut entry = archive.by_name(&item.inside)?;
        let mut total: u64 = 0;
        loop {
            if stop.load(std::sync::atomic::Ordering::Relaxed) {
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
