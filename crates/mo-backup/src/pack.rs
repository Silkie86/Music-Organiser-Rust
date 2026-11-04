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
//! [`crate::gone`] finds them for the page's warning). Its bytes are copied over exactly as they are, never unpacked
//! and packed again, so one damaged in the zip stays as it was (and is named), never dropped. One that's still in your
//! folders but no longer the song's (a sample swapped for another, an autosave the level leaves out) goes from the
//! zip: you still have it. So a zip on the backup is only ever packed over when its list can be read and names this
//! song: one that can't be read may keep files gone from your folders, and stays as it is.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{Datelike, Local, TimeZone, Timelike};
use mo_formats::textscan::{safe_file_name, unsafe_in_file_name};
use mo_space::size::is_sound;
use rayon::prelude::*;
use sha2::{Digest, Sha256};
use zip::result::ZipError;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipArchive, ZipWriter};

use crate::copy::{BUFFER, CantStart, Drive, FAT32_LARGEST, PART, put_in_place, read_some, remove_part};
use crate::gone::{Places, Stamp, stamp};
use crate::parts::Parts;
use crate::plan::{from_backup, place_on_backup};
use crate::record::{nanos, safe_rel};
use crate::stopped;

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
        /// Files kept from the zip before that are damaged in it.
        damaged: Vec<Damaged>,
    },
    /// Nothing in it changed since it was made.
    UpToDate {
        /// The files it keeps that aren't in your folders now.
        kept: usize,
    },
}

/// A kept file found damaged in its song's zip when the song was packed again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Damaged {
    /// Its place inside the zip.
    pub inside: String,
    /// Copied over as it was, as its bytes don't read back right (nothing more is lost, but it may not open). `false`:
    /// the zip's list named it, but the zip doesn't have it, so there was nothing to keep.
    pub kept: bool,
}

/// A zip name for each song, in the order given: its title made safe for every system (and cut short), and a
/// number for a song whose name another song already has, letter case aside. Songs are named in the order of their
/// ids, so a song keeps its name when a newer song of the same title comes along. A name a zip on the backup has for
/// another song (one gone from your folders, say) or that isn't one of the app's is never given: `on_backup` is each
/// zip there, by its name in small letters, and the song it belongs to ([`owners`]).
pub fn pack_names(songs: &[(&str, u64)], on_backup: &HashMap<String, Option<u64>>) -> Vec<String> {
    let mut order: Vec<usize> = (0..songs.len()).collect();
    order.sort_by_key(|&i| songs[i].1);
    let mut taken: HashSet<String> = HashSet::new();
    let mut names = vec![String::new(); songs.len()];
    for i in order {
        let (title, song) = songs[i];
        let base: String = safe_file_name(title).chars().take(LONGEST_TITLE).collect();
        let base = base.trim_end_matches([' ', '.']).to_owned();
        let free = |name: &str| on_backup.get(&name.to_lowercase()).is_none_or(|owner| *owner == Some(song));
        let mut name = format!("{base}.zip");
        let mut n = 2;
        while !free(&name) || !taken.insert(name.to_lowercase()) {
            name = format!("{base} ({n}).zip");
            n += 1;
        }
        names[i] = name;
    }
    names
}

/// Each song zip on the backup by its name in small letters, and the song it belongs to (`None`: it isn't one of the
/// app's, or is damaged, so whose it is can't be told), for [`pack_names`].
pub fn owners(lists: &[ZipList]) -> HashMap<String, Option<u64>> {
    lists.iter().filter_map(|l| Some((l.zip.file_name()?.to_string_lossy().to_lowercase(), l.song))).collect()
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
    /// The zip that was there before: a file in your folders whose bytes match its copy there, though its time moved.
    Same,
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
    /// Asks whether a kept file is back in your folders, each drive once a run.
    places: Places,
    /// This run's half-done zips, cleared by a later run if this one is cut off.
    parts: Parts,
}

impl<'a> Packer<'a> {
    /// Ready to pack onto `backup`, which must already be there (nothing is created in its place).
    pub fn new(backup: &'a Path) -> Result<Packer<'a>, CantStart> {
        let drive = Drive::find(backup).ok_or_else(|| CantStart::NoBackupFolder(backup.to_path_buf()))?;
        // Names inside a zip come from your computer, so they're told apart the computer's way.
        let ignores_case = cfg!(any(windows, target_os = "macos"));
        Ok(Packer { backup, drive, ignores_case, places: Places::default(), parts: Parts::start(backup) })
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
    /// of that name that belongs to another song, or whose list can't be read; the zip already there stays as it was.
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
        let old = match fs::symlink_metadata(&dest) {
            Ok(meta) if meta.is_file() => Some(read_old(&dest, song)?),
            Ok(_) => return Err(io::Error::other(format!("{} isn't a zip, so it's left as it is", dest.display()))),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => return Err(about(&dest, e)),
        };
        // What the zip before holds that isn't in the list now stays, while it isn't in your folders (the system is
        // only asked about those). One it can't tell about (a drive that isn't plugged in) stays too.
        let mut kept = 0;
        for (size, time, inside) in old.iter().flat_map(|o| &o.lines) {
            let key = self.fold(inside);
            if seen.contains(&key) {
                continue;
            }
            if !from_backup(inside).is_some_and(|p| self.places.there(&p)) {
                seen.insert(key);
                items.push(Item { source: Source::Kept, inside: inside.clone(), size: *size, time: *time });
                kept += 1;
            }
        }
        items.sort_by(|a, b| a.inside.cmp(&b.inside));
        let match_before = match &old {
            Some(old) => same(old, &dest, &list_text(song, items.iter()), &items, stop)?,
            None => Match::No,
        };
        match match_before {
            Match::List => return Ok(Packed::UpToDate { kept }),
            Match::Bytes => {
                // Only times moved (a FAT or exFAT drive after the clocks change, a tool that set new times): the zip
                // is made again from its own bytes, not your files, so its list carries the new times and the next run
                // doesn't read every file again to compare them. A drive that's full or can't be written to leaves
                // the zip as it is (it holds the files as they are); anything else (a zip that doesn't read back
                // right) packs the song afresh from your files.
                let quick: Vec<Item> = items
                    .iter()
                    .map(|i| Item {
                        source: match i.source {
                            Source::File(_) => Source::Same,
                            ref other => other.clone(),
                        },
                        inside: i.inside.clone(),
                        size: i.size,
                        time: i.time,
                    })
                    .collect();
                match self.write(&quick, name, song, &dest, stop) {
                    Ok((_, missing, bad)) if missing.is_empty() && bad.is_empty() => {
                        return Ok(Packed::UpToDate { kept });
                    }
                    Ok((bytes, missing, bad)) => return Ok(written(bytes, kept, missing, bad)),
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => return Err(e),
                    Err(e)
                        if matches!(
                            e.kind(),
                            io::ErrorKind::StorageFull
                                | io::ErrorKind::PermissionDenied
                                | io::ErrorKind::ReadOnlyFilesystem
                        ) =>
                    {
                        self.drive_there()?;
                        return Ok(Packed::UpToDate { kept });
                    }
                    Err(_) => self.drive_there()?,
                }
            }
            Match::No => {}
        }
        // A damaged drive can report any size: saturating, never a panic.
        let total = items.iter().map(|i| i.size).fold(0, u64::saturating_add);
        match self.write(&items, name, song, &dest, stop) {
            Ok((bytes, missing, bad)) => Ok(written(bytes, kept, missing, bad)),
            Err(e) if e.kind() == io::ErrorKind::StorageFull && total > FAT32_LARGEST => {
                // On a FAT32 drive a file can't pass 4 GB, and the system calls that a full drive.
                Err(io::Error::other(
                    "This song is over 4 GB, more than one file can hold on a FAT32 drive (or the drive is full)",
                ))
            }
            Err(e) => Err(e),
        }
    }

    /// Writes `items` into the song's zip under its temporary name, reads it back and puts it in place of the zip
    /// before (`dest`). Gives its size, the kept files the zip before doesn't have, and the kept files damaged in it.
    /// On any error the half-made zip is removed and the zip before stays.
    fn write(
        &self,
        items: &[Item],
        name: &str,
        song: u64,
        dest: &Path,
        stop: &AtomicBool,
    ) -> io::Result<(u64, Vec<String>, Vec<String>)> {
        let folder = self.backup.join(SONGS);
        fs::create_dir_all(&folder)?;
        let part = folder.join(format!("{name}{PART}"));
        match fs::symlink_metadata(&part) {
            Ok(m) if m.is_file() => remove_part(&part),
            Ok(_) => return Err(io::Error::other("Something that isn't a file is in the way of the zip")),
            Err(_) => {}
        }
        self.parts.part(&part);
        write_zip(items, song, dest, &part, stop)
            .and_then(|missing| check_zip(&part, items, dest, &missing, stop).map(|(bytes, bad)| (bytes, missing, bad)))
            .and_then(|done| self.drive_there().map(|()| done))
            .and_then(|done| put_in_place(&part, dest).map(|()| done))
            // Reading the zip before notices Stop as some other error.
            .map_err(|e| if stop.load(Ordering::Relaxed) { stopped() } else { e })
            .inspect_err(|_| remove_part(&part))
    }
}

/// What a zip packed again says: the kept files it lost (the zip before didn't have them) and found damaged.
fn written(bytes: u64, kept: usize, missing: Vec<String>, bad: Vec<String>) -> Packed {
    let kept = kept.saturating_sub(missing.len());
    let damaged = bad
        .into_iter()
        .map(|inside| Damaged { inside, kept: true })
        .chain(missing.into_iter().map(|inside| Damaged { inside, kept: false }))
        .collect();
    Packed::Written { bytes, kept, damaged }
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

/// One line of a zip's list (or the backup's record): a file's size, its time (nanoseconds since 1970) and its place
/// inside the zip (or on the backup), with forward slashes.
pub type Line = (u64, i128, String);

/// The list in the zip already on the backup.
struct Old {
    /// As written.
    text: String,
    lines: Vec<Line>,
}

/// Why a zip's list can't be had.
enum Unlisted {
    /// The zip couldn't be read now (a program holding it, a drive problem): tried again next time.
    CantRead(io::Error),
    /// It isn't a whole zip with the app's list in it: damaged, or not one of the app's. Why, in words.
    NotOurs(&'static str),
}

/// The song id and list in a zip.
fn open_list(zip: &Path) -> Result<(u64, String), Unlisted> {
    let damaged = |e: &io::Error| {
        matches!(e.kind(), io::ErrorKind::InvalidData | io::ErrorKind::InvalidInput | io::ErrorKind::UnexpectedEof)
    };
    let file = File::open(zip).map_err(Unlisted::CantRead)?;
    let mut archive = ZipArchive::new(file).map_err(|e| match e {
        ZipError::Io(e) if !damaged(&e) => Unlisted::CantRead(e),
        _ => Unlisted::NotOurs("isn't a whole zip"),
    })?;
    let mut entry = archive.by_name(LIST).map_err(|e| match e {
        ZipError::Io(e) if !damaged(&e) => Unlisted::CantRead(e),
        ZipError::FileNotFound => Unlisted::NotOurs("has no list of the app's in it"),
        _ => Unlisted::NotOurs("has a damaged list"),
    })?;
    let mut text = String::new();
    entry.read_to_string(&mut text).map_err(|e| match damaged(&e) {
        true => Unlisted::NotOurs("has a damaged list"),
        false => Unlisted::CantRead(e),
    })?;
    let song = text.lines().next().and_then(|l| l.strip_prefix("song\t")).and_then(|id| id.parse().ok());
    let song = song.ok_or(Unlisted::NotOurs("has a list that isn't the app's"))?;
    Ok((song, text))
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
    inside != LIST && safe_rel(inside)
}

/// The list of the zip already at `zip`. An error when it's another song's, or its list can't be read: it may keep files
/// gone from your folders, so it's never packed over.
fn read_old(zip: &Path, song: u64) -> io::Result<Old> {
    let (owner, text) = open_list(zip).map_err(|why| match why {
        Unlisted::CantRead(e) => io::Error::new(
            e.kind(),
            format!("{} couldn't be read ({e}), so it's left as it is: it's tried again next time", zip.display()),
        ),
        Unlisted::NotOurs(why) => io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{} {why}, so it's left as it is (it may keep files gone from your folders): move it out of the \
                 Songs folder to pack this song again",
                zip.display()
            ),
        ),
    })?;
    if owner != song {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("{} belongs to another song", zip.display())));
    }
    let lines = text.lines().skip(1).filter_map(parse_line).collect();
    Ok(Old { text, lines })
}

/// How the zip already there compares with the files as they are now.
#[derive(Debug, PartialEq, Eq)]
enum Match {
    /// Its list is the same.
    List,
    /// The same files at the same sizes, with only some times moved, whose bytes match the zip's own check numbers.
    Bytes,
    /// Something in it changed.
    No,
}

/// Whether the zip already there holds the files as they are now: the same list, or the same files at the same sizes
/// with only some times moved (a FAT or exFAT drive after the clocks change, a copy that took new times), whose bytes
/// match the zip's own check numbers.
fn same(old: &Old, zip: &Path, list: &str, items: &[Item], stop: &AtomicBool) -> io::Result<Match> {
    if old.text == list {
        return Ok(Match::List);
    }
    let old_items: HashMap<&str, u64> = old.lines.iter().map(|(size, _, inside)| (inside.as_str(), *size)).collect();
    if old_items.len() != items.len() || items.iter().any(|i| old_items.get(i.inside.as_str()) != Some(&i.size)) {
        return Ok(Match::No);
    }
    let Ok(file) = File::open(zip) else { return Ok(Match::No) };
    let Ok(mut archive) = ZipArchive::new(file) else { return Ok(Match::No) };
    let mut buf = vec![0u8; BUFFER];
    for item in items {
        // A kept file's line is the zip's own.
        let Source::File(source) = &item.source else { continue };
        let Ok(entry) = archive.by_name(&item.inside) else { return Ok(Match::No) };
        let crc = entry.crc32();
        drop(entry);
        let mut src = File::open(source).map_err(|e| about(source, e))?;
        let mut hasher = crc32fast::Hasher::new();
        while let Some(n) = read_some(&mut src, &mut buf, stop).map_err(|e| about(source, e))? {
            hasher.update(&buf[..n]);
        }
        if hasher.finalize() != crc {
            return Ok(Match::No);
        }
    }
    Ok(Match::Bytes)
}

/// One song zip on the backup: the zip, the song it belongs to, and each file its list names.
#[derive(Clone, Debug)]
pub struct ZipList {
    pub zip: PathBuf,
    /// The song's id; `None` for a zip that isn't one of the app's, or is damaged (it names no files then).
    pub song: Option<u64>,
    pub files: Vec<Line>,
    /// The zip's size and time when its list was read.
    stamp: Option<Stamp>,
}

/// The lists of every song zip on the backup, in order of the zips, several read at a time on the current rayon pool.
/// A zip that can't be read now is left out; no Songs folder, none. Stopped, an `Interrupted` error.
pub fn lists(backup: &Path, stop: &AtomicBool) -> io::Result<Vec<ZipList>> {
    let lists = lists_reusing(backup, Vec::new(), stop)?;
    if stop.load(Ordering::Relaxed) {
        return Err(stopped());
    }
    Ok(lists)
}

/// As [`lists`], reusing each list in `before` whose zip is the same size and time as when it was read. Stopped part
/// way, it gives the lists it has (each one whole), and the rest are read next time.
pub(crate) fn lists_reusing(backup: &Path, before: Vec<ZipList>, stop: &AtomicBool) -> io::Result<Vec<ZipList>> {
    let entries = match fs::read_dir(backup.join(SONGS)) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut before: HashMap<PathBuf, ZipList> = before.into_iter().map(|l| (l.zip.clone(), l)).collect();
    let zips: Vec<(PathBuf, Option<Stamp>, Option<ZipList>)> = entries
        .filter_map(Result::ok)
        // Not through links: only the zips really in the Songs folder.
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter(|e| Path::new(&e.file_name()).extension().is_some_and(|x| x.eq_ignore_ascii_case("zip")))
        .map(|e| {
            let zip = e.path();
            let now = e.metadata().ok().and_then(|m| stamp(&m));
            let had = before.remove(&zip).filter(|l| now.is_some() && l.stamp == now);
            (zip, now, had)
        })
        .collect();
    let mut lists: Vec<ZipList> = zips
        .into_par_iter()
        .filter_map(|(zip, now, had)| {
            if had.is_some() || stop.load(Ordering::Relaxed) {
                return had;
            }
            match open_list(&zip) {
                Ok((song, text)) => {
                    let files = text.lines().skip(1).filter_map(parse_line).collect();
                    Some(ZipList { zip, song: Some(song), files, stamp: now })
                }
                // Looked at again each time, as cheap as it is rare.
                Err(Unlisted::NotOurs(_)) => Some(ZipList { zip, song: None, files: Vec::new(), stamp: None }),
                // Read again next time.
                Err(Unlisted::CantRead(_)) => None,
            }
        })
        .collect();
    lists.sort_by(|a, b| a.zip.cmp(&b.zip));
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

/// The zip before, read for its kept files' bytes. Stop is noticed between reads (a raw copy can't be told), as an
/// error that isn't `Interrupted`, which readers try again on: the packer says it stopped.
struct Stoppable<'a> {
    file: File,
    stop: &'a AtomicBool,
}

impl Read for Stoppable<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.stop.load(Ordering::Relaxed) {
            return Err(io::Error::other("Stopped"));
        }
        self.file.read(buf)
    }
}

impl Seek for Stoppable<'_> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.file.seek(pos)
    }
}

/// Writes the zip under its temporary name: each file from your folders, each kept one from the zip before (`old`),
/// then the list. A kept file's bytes are copied as they are in the zip before (not unpacked and packed again), so
/// nothing about them changes, damaged or not. Gives the kept files the zip before's list names but that aren't in it
/// at all, which are left out of the new list; anything else that goes wrong fails the zip, and the zip before stays.
fn write_zip(items: &[Item], song: u64, old: &Path, part: &Path, stop: &AtomicBool) -> io::Result<Vec<String>> {
    let file = File::create_new(part)?;
    let mut zip = ZipWriter::new(io::BufWriter::new(file));
    let mut buf = vec![0u8; BUFFER];
    // The zip before, when a file is taken from it.
    let mut before = match items.iter().any(|i| matches!(i.source, Source::Kept | Source::Same)) {
        true => Some(ZipArchive::new(Stoppable { file: File::open(old)?, stop })?),
        false => None,
    };
    let mut missing: Vec<String> = Vec::new();
    for item in items {
        match &item.source {
            Source::File(source) => {
                // Sounds (audio, sample-library files, anything big) barely shrink: stored as they are. Projects,
                // presets and MIDI shrink a lot.
                let method = match is_sound(&item.inside, item.size) {
                    true => CompressionMethod::Stored,
                    false => CompressionMethod::Deflated,
                };
                let mut options =
                    SimpleFileOptions::default().compression_method(method).large_file(item.size >= LARGE);
                if let Some(when) = zip_time(item.time) {
                    options = options.last_modified_time(when);
                }
                zip.start_file(item.inside.as_str(), options)?;
                copy_file(source, item, &mut zip, &mut buf, stop)?;
            }
            Source::Same => {
                // Its bytes as they are in the zip before (same() matched them), with its new time.
                let archive = before.as_mut().ok_or_else(|| io::Error::other("The zip before couldn't be opened"))?;
                let index = archive
                    .index_for_name(&item.inside)
                    .ok_or_else(|| io::Error::other(format!("{} isn't in the zip before", item.inside)))?;
                let entry = archive.by_index_raw(index)?;
                match zip_time(item.time) {
                    Some(when) => zip.raw_copy_file_touch(entry, when, None)?,
                    None => zip.raw_copy_file(entry)?,
                }
            }
            Source::Kept => {
                let Some(archive) = before.as_mut() else {
                    return Err(io::Error::other("The zip before couldn't be opened"));
                };
                let Some(index) = archive.index_for_name(&item.inside) else {
                    missing.push(item.inside.clone());
                    continue;
                };
                // Its place in the zip before can't be found (damage where it starts): it can't be copied, and the
                // zip isn't packed over without it.
                let entry = archive.by_index_raw(index).map_err(|e| match e {
                    ZipError::Io(e) => e,
                    e => io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "{} is damaged where it keeps {} ({e}), a file gone from your folders, so it's left as it \
                             is: move it out of the Songs folder to pack this song again",
                            old.display(),
                            item.inside
                        ),
                    ),
                })?;
                zip.raw_copy_file(entry)?;
            }
        }
    }
    zip.start_file(LIST, SimpleFileOptions::default())?;
    zip.write_all(list_text(song, items.iter().filter(|i| !missing.contains(&i.inside))).as_bytes())?;
    let out = zip.finish()?;
    out.into_inner().map_err(io::IntoInnerError::into_error)?.sync_all()?;
    Ok(missing)
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

/// Reads every entry of the new zip back to the end (the zip checks each one's bytes against the check number it
/// stored) and checks each is the size it should be. A kept file that doesn't read back right is only let through when
/// its bytes are exactly those in the zip before (`old`): it was damaged there already, and is kept as it was. Gives the
/// zip's size and those files' places. `missing` is the kept files left out because the zip before doesn't have them.
fn check_zip(
    part: &Path,
    items: &[Item],
    old: &Path,
    missing: &[String],
    stop: &AtomicBool,
) -> io::Result<(u64, Vec<String>)> {
    let mut archive = ZipArchive::new(File::open(part)?)?;
    let items: Vec<&Item> = items.iter().filter(|i| !missing.contains(&i.inside)).collect();
    if archive.len() != items.len() + 1 {
        return Err(io::Error::other("The zip read back with the wrong number of files"));
    }
    let mut buf = vec![0u8; BUFFER];
    let mut before: Option<ZipArchive<File>> = None;
    let mut damaged = Vec::new();
    for item in items {
        let Err(e) = read_back(&mut archive, item, &mut buf, stop) else { continue };
        if e.kind() == io::ErrorKind::Interrupted {
            return Err(e);
        }
        if item.source == Source::Kept {
            let before = match &mut before {
                Some(before) => before,
                None => before.insert(ZipArchive::new(File::open(old)?)?),
            };
            if raw_bytes_sum(before, &item.inside, &mut buf, stop)?
                == raw_bytes_sum(&mut archive, &item.inside, &mut buf, stop)?
            {
                damaged.push(item.inside.clone());
                continue;
            }
        }
        return Err(io::Error::other(format!(
            "{} read back wrongly ({e}): the backup drive may be failing",
            item.inside
        )));
    }
    Ok((fs::metadata(part)?.len(), damaged))
}

/// Reads one entry of a zip to the end, which checks its bytes against its check number, and checks its size.
fn read_back(archive: &mut ZipArchive<File>, item: &Item, buf: &mut [u8], stop: &AtomicBool) -> io::Result<()> {
    let mut entry = archive.by_name(&item.inside)?;
    let mut total: u64 = 0;
    loop {
        if stop.load(Ordering::Relaxed) {
            return Err(stopped());
        }
        match entry.read(buf) {
            Ok(0) => break,
            Ok(n) => total = total.saturating_add(n as u64),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    if total != item.size {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("{total} bytes, not {}", item.size)));
    }
    Ok(())
}

/// The SHA-256 of an entry's bytes as they're stored in a zip (not unpacked), and how many there are.
fn raw_bytes_sum(
    archive: &mut ZipArchive<File>,
    inside: &str,
    buf: &mut [u8],
    stop: &AtomicBool,
) -> io::Result<([u8; 32], u64)> {
    let index = archive.index_for_name(inside).ok_or_else(|| io::Error::other(format!("{inside} isn't in the zip")))?;
    let mut entry = archive.by_index_raw(index)?;
    let mut hash = Sha256::new();
    let mut total: u64 = 0;
    loop {
        if stop.load(Ordering::Relaxed) {
            return Err(stopped());
        }
        match entry.read(buf) {
            Ok(0) => break,
            Ok(n) => {
                hash.update(&buf[..n]);
                total = total.saturating_add(n as u64);
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok((hash.finalize().into(), total))
}
