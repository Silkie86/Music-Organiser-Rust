//! Archiving the old app's databases (step 6 of the new databases; Solomon, 2026-11-05: "once the new Rust files have
//! worked for a week", "moved to an archive folder", "never deleted").
//!
//! The C# app kept everything in `library.db`, `sounds.db` and `midi.db`. This app has its own (`songs.db`,
//! `sounds-rust.db`, `midi-index.db`), each made from the old one, and once it has taken everything it needs from an
//! old one it never opens it again. A little after each start this looks whether that's so, and has been for a week
//! ([`ready_at`]), and then puts the old database away in [`FOLDER`], a folder of its own in the data folder, with the
//! files SQLite keeps beside it (`library.db-wal` holds its newest changes: without it the database is behind):
//!
//! - One in the data folder is **moved** there (a rename on the same drive: nothing is read or written).
//! - One that stayed in the folder the data was moved out of ([`crate::data_move`]: `sounds.db` wasn't copied along)
//!   is **copied** there, read back and checked against what was read, and the one in the old folder is left as it is:
//!   that folder is yours to remove, and its note is written again to say nothing in it is waited for any more.
//!
//! Nothing is ever deleted or replaced: a name the archive already holds stays as it is, and so does the file that
//! would have gone there. It's done one old database at a time, each only once its own replacement is ready, so a
//! page you haven't opened yet (the MIDI index takes the old corrections in when the MIDI page first shows) keeps its
//! old database until it has. A move or copy cut short (the app closed, a file held by another program) is gone on
//! with at the next start; a half-written copy has a name of its own ([`is_part`]) that the start-up sweep clears
//! ([`crate::tidy`], rule 28).

use std::fs::{self, File, Metadata};
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::activity::{Feed, Job};
use crate::log;

/// The folder in the data folder the old databases are put away in (the Space page counts it as the old app's
/// databases by this name).
pub const FOLDER: &str = mo_space::app_files::OLD_DATABASES_FOLDER;

/// A few words in [`FOLDER`] on what's in it, with a line for each database put there.
pub const READ_ME: &str = "READ ME.txt";

/// The old app's databases, by name.
pub const OLD: [&str; 3] = mo_space::app_files::OLD_DATABASE_FILES;

/// How long the app's own database must have done without the old one before that's put away.
pub const WORKED_FOR: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// How long after the start it waits: after the pages' first reads, and after the song library is made
/// ([`crate::song_library`]).
const WAIT: Duration = Duration::from_secs(90);

/// Room left free on the drive beyond a copy.
const SPARE_ROOM: u64 = 1 << 30;

/// A database's own file, then the files SQLite keeps beside it.
const SIDES: [&str; 4] = mo_space::app_files::JOURNALS;

/// What a copy is called while it's written and checked, after the file's own name and before the app's process
/// number ([`is_part`]).
const PART: &str = ".part-";

const BUFFER: usize = 1 << 20;

/// Whether the old database `old` (one of [`OLD`]) in the data folder `data` may be put away at `now` (Unix
/// seconds): the app's own has taken everything it needs from it, and has done without it for [`WORKED_FOR`]. Says
/// why not otherwise, for the log.
///
/// - `library.db`: `songs.db` is the app's own song library, and the one thing still read from `library.db` after
///   that, the FL Studio saves whose muted marks were lost ([`mo_library::song_writes::to_mend`]), has been.
/// - `sounds.db`: `sounds-rust.db` is the app's own sound library, and its backup copy is there and whole
///   ([`mo_sounds::compact::backup::usable`], which reads all of it: asked only once the week is up): `sounds.db`
///   was what a damaged library with no usable backup was made from again.
/// - `midi.db`: the MIDI index is in this version's layout and has taken in the corrections you made in the old app.
pub fn ready_at(data: &Path, old: &str, now: i64) -> Result<(), String> {
    if old == mo_library::songs_db::OLD_FILE_NAME {
        let songs = data.join(mo_library::songs_db::FILE_NAME);
        let since = mo_library::songs_db::own_since(&songs)
            .ok_or_else(|| "songs.db isn't the app's own song library yet".to_owned())?;
        worked(since, now, "songs.db has been the app's own song library")?;
        if !mo_library::song_writes::mend_noted(&songs) {
            return Err("the FL Studio saves whose muted samples are mended from it haven't all been read yet".into());
        }
        Ok(())
    } else if old == "sounds.db" {
        let copy = data.join(mo_sounds::compact::FILE_NAME);
        let since = mo_sounds::compact::own_since(&copy)
            .ok_or_else(|| "sounds-rust.db isn't the app's own sound library yet".to_owned())?;
        worked(since, now, "sounds-rust.db has been the app's own sound library")?;
        mo_sounds::compact::backup::usable(&copy)
    } else if old == "midi.db" {
        let since = mo_midi::store::took_in_old_at(&data.join(mo_midi::store::FILE_NAME)).ok_or_else(|| {
            "the MIDI index hasn't taken in the old app's corrections yet (it does when the MIDI page first shows)"
                .to_owned()
        })?;
        worked(since, now, "the MIDI index has had the old app's corrections")
    } else {
        Err("it isn't one of the old app's databases".into())
    }
}

/// Whether what began at `since` has gone on for [`WORKED_FOR`] at `now` (both Unix seconds); `what` says what, for
/// the reason why not ("songs.db has been the app's own song library for 3 days: it's put away after 7").
fn worked(since: i64, now: i64, what: &str) -> Result<(), String> {
    let day = 24 * 60 * 60;
    let week = WORKED_FOR.as_secs() as i64;
    if since > now {
        return Err(format!("{what} since a time after now (was the clock changed?)"));
    }
    let days = (now - since) / day;
    if now - since < week {
        let days = if days == 1 { "1 day".to_owned() } else { format!("{days} days") };
        return Err(format!("{what} for {days}: it's put away after {}", week / day));
    }
    Ok(())
}

/// What one look did ([`archive`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Archived {
    /// Old databases moved into [`FOLDER`] from the data folder, each with what it and its side files take.
    pub moved: Vec<(&'static str, u64)>,
    /// Old databases copied into [`FOLDER`] from a folder the data was moved out of.
    pub copied: Vec<Copied>,
    /// Why an old database that's there stays where it is for now.
    pub waits: Vec<String>,
    /// What couldn't be done this time; it's tried again at the next start.
    pub failed: Vec<String>,
}

impl Archived {
    /// Whether anything was put away.
    pub fn any(&self) -> bool {
        !self.moved.is_empty() || !self.copied.is_empty()
    }
}

/// An old database copied from a folder the data was moved out of: its name, what it and its side files take, and
/// the folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Copied {
    pub name: &'static str,
    pub bytes: u64,
    pub from: PathBuf,
}

/// How far a copy from the old folder is: the database's name, and the bytes done of those to do.
pub struct Copying<'a> {
    pub name: &'a str,
    pub done: u64,
    pub total: u64,
}

/// Puts away each old database that `ready` says may go (as [`ready_at`] does): those in the data folder `data` are
/// moved into its [`FOLDER`]; one only in one of `old_folders` (the folders the data was moved out of, the last
/// first: [`old_folders`]) is copied there and checked, and left where it is. `copying` hears how far a copy is.
/// Reads and writes files, so never on the window's thread.
pub fn archive(
    data: &Path,
    old_folders: &[PathBuf],
    ready: &dyn Fn(&str) -> Result<(), String>,
    copying: &mut dyn FnMut(Copying<'_>),
) -> Archived {
    let mut done = Archived::default();
    let archive = data.join(FOLDER);
    for name in OLD {
        let here = family(data, name);
        let kept = put_away(data, name).is_some();
        let main_here = here.first().is_some_and(|f| f.side.is_empty());
        if main_here && kept {
            done.waits.push(format!(
                "{name} is in the data folder again, and {FOLDER} already holds one: both stay as they are"
            ));
        } else if main_here {
            match ready(name).and_then(|()| folder_ready(&archive)) {
                Ok(()) => move_in(&here, &archive, name, &mut done),
                Err(why) => done.waits.push(format!("{name} stays where it is for now: {why}")),
            }
        } else if kept && !here.is_empty() {
            // Its side files, left when a move was cut short: they follow it.
            move_in(&here, &archive, name, &mut done);
        } else if !kept {
            // Only what a move left in an old folder: one the data folder has (or had) is the newer. The folder the
            // data was in last comes first: it had the newest.
            let found = old_folders.iter().find_map(|folder| {
                let there = family(folder, name);
                there.first().is_some_and(|f| f.side.is_empty()).then_some((folder, there))
            });
            if let Some((old_folder, there)) = found {
                match ready(name).and_then(|()| folder_ready(&archive)) {
                    Ok(()) => match copy_in(&there, &archive, name, copying) {
                        Ok(bytes) => {
                            done.copied.push(Copied { name, bytes, from: old_folder.clone() });
                            note_in_read_me(&archive, name, "copied here", old_folder);
                        }
                        Err(why) => done.failed.push(format!(
                            "{name} couldn't be copied from {} ({why}); it's tried again next start",
                            old_folder.display()
                        )),
                    },
                    Err(why) => done.waits.push(format!("{name} stays in {} for now: {why}", old_folder.display())),
                }
            }
        }
    }
    done
}

/// Whether the old file `file` (a database of [`OLD`], or a file SQLite keeps beside one) has been put away from the
/// data folder `data`: its database is in [`FOLDER`].
pub fn holds(data: &Path, file: &str) -> bool {
    OLD.iter()
        .find(|old| file.strip_prefix(**old).is_some_and(|side| SIDES.contains(&side)))
        .is_some_and(|old| put_away(data, old).is_some())
}

/// The file `file` in the data folder `data`'s [`FOLDER`], when it's a plain file in a real folder: never looked for
/// through a link where the folder should be.
fn put_away(data: &Path, file: &str) -> Option<Metadata> {
    let archive = data.join(FOLDER);
    fs::symlink_metadata(&archive).ok().filter(|m| m.file_type().is_dir())?;
    plain_file(&archive.join(file))
}

/// The folders the data in `data` was moved out of, the last first, as each folder's own record of its move says
/// ([`crate::data_move::moved_in`]): a second move made before an old database was put away leaves it two folders
/// back.
pub fn old_folders(data: &Path) -> Vec<PathBuf> {
    let mut folders: Vec<PathBuf> = Vec::new();
    let mut at = data.to_path_buf();
    while folders.len() < 8
        && let Some(moved) = crate::data_move::moved_in(&at)
    {
        let from = PathBuf::from(moved.from);
        if !from.is_absolute() || from == data || folders.contains(&from) {
            break;
        }
        folders.push(from.clone());
        at = from;
    }
    folders
}

/// Whether `name`, in [`FOLDER`], is a copy that was being written when the app stopped: one of the old files' own
/// names, [`PART`], then a process number. The start-up sweep clears one that's old ([`crate::tidy`]).
pub fn is_part(name: &str) -> bool {
    name.rsplit_once(PART).is_some_and(|(file, number)| {
        crate::tidy::is_number(number)
            && OLD.iter().any(|old| file.strip_prefix(old).is_some_and(|side| SIDES.contains(&side)))
    })
}

/// One file of a database: which ([`SIDES`]), where, and its size and times.
struct Member {
    side: &'static str,
    path: PathBuf,
    meta: Metadata,
}

/// The files of the database `name` there are in `folder`, its own first: plain files only, never a link.
fn family(folder: &Path, name: &str) -> Vec<Member> {
    SIDES
        .iter()
        .filter_map(|&side| {
            let path = folder.join(format!("{name}{side}"));
            plain_file(&path).map(|meta| Member { side, path, meta })
        })
        .collect()
}

/// A plain file's size and times: `None` for a link, a folder, or nothing there.
fn plain_file(path: &Path) -> Option<Metadata> {
    fs::symlink_metadata(path).ok().filter(Metadata::is_file)
}

/// Makes the archive folder if it isn't there. One that's a link (or a file) is refused: the old databases are only
/// ever put in a real folder of the data folder's own.
fn folder_ready(archive: &Path) -> Result<(), String> {
    match fs::symlink_metadata(archive) {
        Ok(meta) if meta.file_type().is_dir() => Ok(()),
        Ok(_) => Err(format!("{} isn't a plain folder", archive.display())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            fs::create_dir(archive).map_err(|e| format!("{} couldn't be made ({e})", archive.display()))
        }
        Err(e) => Err(format!("{} couldn't be looked at ({e})", archive.display())),
    }
}

/// Moves the files `here` of the database `name` into `archive`, its own first: once that has gone nothing can be
/// holding the others (a program that has a database open holds its own file). Never over a file that's there.
fn move_in(here: &[Member], archive: &Path, name: &'static str, done: &mut Archived) {
    let mut bytes = 0u64;
    let mut moved = false;
    for member in here {
        let file = format!("{name}{}", member.side);
        let to = archive.join(&file);
        let failed = if fs::symlink_metadata(&to).is_ok() {
            Some(format!("{FOLDER} already has a {file}, so the one in the data folder stays where it is"))
        } else {
            match fs::rename(&member.path, &to) {
                Ok(()) => None,
                // Gone meanwhile: another copy of the app, started with this one, has just moved it.
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    if member.side.is_empty() {
                        break;
                    }
                    continue;
                }
                Err(e) => Some(format!("{file} couldn't be moved into {FOLDER} ({e}); it's tried again next start")),
            }
        };
        match failed {
            None => {
                moved = true;
                bytes = bytes.saturating_add(member.meta.len());
            }
            Some(why) => {
                done.failed.push(why);
                // Without its own file moved, its side files stay with it.
                if member.side.is_empty() {
                    break;
                }
            }
        }
    }
    if moved {
        done.moved.push((name, bytes));
        if let Some(from) = here.first().and_then(|m| m.path.parent()) {
            note_in_read_me(archive, name, "moved here", from);
        }
    }
}

/// Copies the files `there` of the database `name` into `archive`, its own last, so the archive holds the database
/// only once all of it is there. Each is read once, written under a name of its own ([`PART`]), read back and
/// checked, and only then given its name; the files copied from aren't changed. What it and its side files take.
fn copy_in(there: &[Member], archive: &Path, name: &str, copying: &mut dyn FnMut(Copying<'_>)) -> Result<u64, String> {
    let total = there.iter().fold(0u64, |sum, m| sum.saturating_add(m.meta.len()));
    let needed = total.saturating_add(SPARE_ROOM);
    if let Some(free) = crate::data_move::free_room(archive)
        && free < needed
    {
        return Err(format!(
            "there isn't room on the drive: it needs about {} and has {} free",
            mo_space::size_text(needed),
            mo_space::size_text(free)
        ));
    }
    let mut done = 0u64;
    for member in there.iter().rev() {
        let file = format!("{name}{}", member.side);
        let to = archive.join(&file);
        match fs::symlink_metadata(&to) {
            // Copied before the app stopped last time (a copy is named only once it's checked, with its time).
            Ok(meta) if meta.is_file() && same(&meta, &member.meta) => {}
            Ok(_) => return Err(format!("{FOLDER} already has another {file}")),
            Err(_) => {
                copy_checked(member, &to, &mut |bytes| copying(Copying { name, done: done + bytes, total }))
                    .map_err(|e| format!("{file}: {e}"))?;
            }
        }
        done = done.saturating_add(member.meta.len());
        copying(Copying { name, done, total });
    }
    Ok(total)
}

/// Whether a copy `a` is of the file `b` as it is now, by size and time (to 2 seconds: some drives keep no finer).
fn same(a: &Metadata, b: &Metadata) -> bool {
    let apart = |a: SystemTime, b: SystemTime| a.duration_since(b).or_else(|_| b.duration_since(a)).ok();
    a.len() == b.len()
        && matches!((a.modified(), b.modified()), (Ok(a), Ok(b)) if apart(a, b).is_some_and(|d| d <= Duration::from_secs(2)))
}

/// Copies one file to `to`: read once, the copy read back and checked against what was read, the file looked at
/// again (one that changed meanwhile isn't a copy of anything), and only then named, with the file's own time.
fn copy_checked(member: &Member, to: &Path, progress: &mut dyn FnMut(u64)) -> io::Result<()> {
    let mut part = to.as_os_str().to_owned();
    part.push(format!("{PART}{}", std::process::id()));
    let part = PathBuf::from(part);
    // One this run left when an earlier try failed: ours by its name.
    let _ = fs::remove_file(&part);
    let result = (|| {
        let mut from = File::open(&member.path)?;
        let mut out = File::create_new(&part)?;
        let mut hash = Sha256::new();
        let mut buf = vec![0u8; BUFFER];
        let mut total = 0u64;
        loop {
            let n = match from.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            };
            hash.update(&buf[..n]);
            out.write_all(&buf[..n])?;
            total += n as u64;
            progress(total);
        }
        if let Ok(time) = member.meta.modified() {
            out.set_modified(time)?;
        }
        out.sync_all()?;
        drop(out);
        drop(from);
        let now = fs::symlink_metadata(&member.path)?;
        if total != member.meta.len() || now.len() != total || now.modified().ok() != member.meta.modified().ok() {
            return Err(io::Error::other("it changed while it was copied"));
        }
        let wrote: [u8; 32] = hash.finalize().into();
        if hash_of(&part, &mut buf)? != wrote {
            return Err(io::Error::other("the copy read back differently from what was read"));
        }
        fs::rename(&part, to)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&part);
    }
    result
}

fn hash_of(path: &Path, buf: &mut [u8]) -> io::Result<[u8; 32]> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    loop {
        match file.read(buf) {
            Ok(0) => return Ok(hash.finalize().into()),
            Ok(n) => hash.update(&buf[..n]),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
}

/// Adds a line to [`READ_ME`] in `archive`: the database `name` was `how` ("moved here", "copied here") from `from`,
/// and when. The first line written says what the folder is. Only a note for whoever opens the folder: one that
/// can't be written changes nothing.
fn note_in_read_me(archive: &Path, name: &str, how: &str, from: &Path) {
    let path = archive.join(READ_ME);
    // Never through a link, nor into anything that isn't a plain file.
    let new = match fs::symlink_metadata(&path) {
        Ok(meta) if meta.is_file() => meta.len() == 0,
        Ok(_) => return,
        Err(_) => true,
    };
    let Ok(mut file) = File::options().create(true).append(true).open(&path) else { return };
    let mut text = String::new();
    if new {
        text.push_str(
            "Music Organiser put the old app's databases here once its own had worked without them for a week.\r\n\
             It doesn't open them any more, and it never deletes them: they're yours to keep or remove.\r\n\
             Each is whole only with the -wal and -shm files of the same name beside it.\r\n\r\n",
        );
    }
    let when = chrono::Local::now().format("%Y-%m-%d %H:%M");
    text.push_str(&format!("{when}  {name}: {how} from {}\r\n", from.display()));
    let _ = file.write_all(text.as_bytes());
}

/// Starts the look on a low-priority thread of its own, [`WAIT`] after the app opens: the old databases in the data
/// folder `data`, and those left in the folders the data was moved out of. Not in a run that uses the old copy of
/// its data (the folder a move went to is away: [`crate::paths::problem`]): that folder is the one the old databases
/// are copied *from*, and nothing in it is changed. The caller leaves it out too in a run that reads `library.db`.
pub fn archive_later(data: PathBuf, feed: Feed) {
    if crate::paths::problem().is_some() {
        return;
    }
    let started = crate::background::spawn("Old databases", move || {
        std::thread::sleep(WAIT);
        archive_now(&data, &old_folders(&data), &crate::paths::first_dir(), &feed);
    });
    if let Err(e) = started {
        log::warning("Old databases", &format!("Couldn't start putting the old app's databases away: {e}"));
    }
}

/// Looks now ([`archive`], with [`ready_at`]), saying in the log what it did and why anything stays, with a card
/// while it copies and one that stays, to say what was put away. `first` is the system's own data folder
/// ([`crate::paths::first_dir`]), for the old folders' notes.
pub fn archive_now(data: &Path, old_folders: &[PathBuf], first: &Path, feed: &Feed) -> Archived {
    // A copy reads gigabytes: they shouldn't push the pages' own out of memory.
    crate::background::lower_memory_priority();
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let mut job: Option<Job> = None;
    let done = archive(data, old_folders, &|old| ready_at(data, old, now), &mut |at| {
        let job = job.get_or_insert_with(|| {
            feed.start(
                "Putting the old app's databases away",
                &format!("Copying them into {} and checking each copy", data.join(FOLDER).display()),
            )
        });
        let detail = format!("Copying {} ({})", at.name, mo_space::size_text(at.total));
        job.progress(if at.total == 0 { 1.0 } else { (at.done as f64 / at.total as f64) as f32 }, Some(&detail));
    });
    if let Some(job) = job {
        // Any copy that failed shows, even beside one that didn't.
        if done.failed.is_empty() {
            job.done("Copied and checked");
        } else {
            job.failed(&done.failed.join("; "));
        }
    }
    for line in &done.waits {
        log::info("Old databases", line);
    }
    for line in &done.failed {
        log::warning("Old databases", line);
    }
    // An old folder's note says to keep what's still to be put away: written again when that's no longer so.
    let mut moved_into = data;
    for old in old_folders {
        if crate::data_move::renew_moved_out_note(moved_into, data, first) {
            log::info("Old databases", &format!("Wrote the note in {} again, as things are now", old.display()));
        }
        moved_into = old;
    }
    if let Some(detail) = card_line(&done, data, old_folders) {
        feed.told("Put the old app's databases away", &detail);
    }
    done
}

/// What the card says once something was put away: what was moved and copied, where to, and that the old folders
/// aren't needed any more once nothing in them is still to be put away. `None` when nothing was.
pub fn card_line(done: &Archived, data: &Path, old_folders: &[PathBuf]) -> Option<String> {
    if !done.any() {
        return None;
    }
    let list = |files: &[(&'static str, u64)]| {
        let names: Vec<&str> = files.iter().map(|&(name, _)| name).collect();
        let bytes = files.iter().fold(0u64, |sum, &(_, bytes)| sum.saturating_add(bytes));
        format!("{} ({})", and(&names), mo_space::size_text(bytes))
    };
    let archive = data.join(FOLDER);
    let mut line = String::new();
    if !done.moved.is_empty() {
        line.push_str(&format!("{} moved into {}. ", list(&done.moved), archive.display()));
    }
    let mut named = !done.moved.is_empty();
    for folder in old_folders {
        let from: Vec<(&'static str, u64)> =
            done.copied.iter().filter(|c| &c.from == folder).map(|c| (c.name, c.bytes)).collect();
        if !from.is_empty() {
            let to = if named { "there".to_owned() } else { format!("into {}", archive.display()) };
            line.push_str(&format!("{} copied {to} from {} and checked. ", list(&from), folder.display()));
            named = true;
        }
    }
    line.push_str("The app has its own now and doesn't open them any more. Nothing was deleted.");
    // One the data folder has too is the newer: the old folder's isn't waited for.
    let waited = |old: &Path| {
        OLD.iter().any(|name| {
            plain_file(&old.join(name)).is_some() && !holds(data, name) && plain_file(&data.join(name)).is_none()
        })
    };
    let free: Vec<String> =
        old_folders.iter().filter(|old| old.is_dir() && !waited(old)).map(|old| old.display().to_string()).collect();
    if !free.is_empty() && free.len() == old_folders.iter().filter(|old| old.is_dir()).count() {
        let free: Vec<&str> = free.iter().map(String::as_str).collect();
        line.push_str(&format!(
            " Nothing in {} is waited for any more: the note in it says what to keep, and the rest can go.",
            and(&free)
        ));
    }
    Some(line)
}

/// "a", "a and b", "a, b and c".
fn and(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [most @ .., last] => format!("{} and {last}", most.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("mo-old-databases-test").join(format!("{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn names(folder: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(folder)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn quiet(_: Copying<'_>) {}

    const READY: &dyn Fn(&str) -> Result<(), String> = &|_| Ok(());

    #[test]
    fn nothing_there_is_nothing_to_do() {
        let data = test_folder("nothing");
        let old = test_folder("nothing-old");
        assert_eq!(archive(&data, std::slice::from_ref(&old), READY, &mut quiet), Archived::default());
        assert_eq!(archive(&data, &[], READY, &mut quiet), Archived::default());
        assert!(names(&data).is_empty(), "no folder is made for nothing");
        assert_eq!(card_line(&Archived::default(), &data, std::slice::from_ref(&old)), None);
    }

    #[test]
    fn an_old_database_moves_with_its_side_files_once_its_ready_and_waits_until_then() {
        let data = test_folder("moves");
        fs::write(data.join("library.db"), b"library").unwrap();
        fs::write(data.join("library.db-wal"), b"newest").unwrap();
        fs::write(data.join("library.db-shm"), b"index").unwrap();
        fs::write(data.join("midi.db"), b"midi").unwrap();
        fs::write(data.join("songs.db"), b"the app's own").unwrap();
        // Only library.db is ready.
        let ready = |old: &str| if old == "library.db" { Ok(()) } else { Err("not yet".to_owned()) };
        let done = archive(&data, &[], &ready, &mut quiet);
        assert_eq!(done.moved, [("library.db", 18)]);
        assert_eq!(done.waits, ["midi.db stays where it is for now: not yet"]);
        assert!(done.copied.is_empty() && done.failed.is_empty(), "{done:?}");
        assert_eq!(names(&data), ["midi.db", FOLDER, "songs.db"]);
        let archive_folder = data.join(FOLDER);
        assert_eq!(names(&archive_folder), [READ_ME, "library.db", "library.db-shm", "library.db-wal"]);
        assert_eq!(fs::read(archive_folder.join("library.db-wal")).unwrap(), b"newest");
        let read_me = fs::read_to_string(archive_folder.join(READ_ME)).unwrap();
        assert!(read_me.starts_with("Music Organiser put the old app's databases here"), "{read_me}");
        assert!(read_me.contains(&format!("library.db: moved here from {}", data.display())), "{read_me}");
        assert!(holds(&data, "library.db") && holds(&data, "library.db-wal") && !holds(&data, "midi.db"));
        assert!(!holds(&data, "library.db.bak") && !holds(&data, "songs.db"));
        // Nothing more to do for it; midi.db goes when its time comes, and the note gets a line, not a new start.
        assert_eq!(archive(&data, &[], &ready, &mut quiet).moved, []);
        let done = archive(&data, &[], READY, &mut quiet);
        assert_eq!(done.moved, [("midi.db", 4)]);
        let read_me = fs::read_to_string(archive_folder.join(READ_ME)).unwrap();
        assert_eq!(read_me.matches("Music Organiser put").count(), 1, "{read_me}");
        assert_eq!(read_me.matches("moved here").count(), 2, "{read_me}");
        assert_eq!(names(&data), [FOLDER, "songs.db"]);
        let line = card_line(&done, &data, &[]).unwrap();
        assert!(line.starts_with("midi.db (4 bytes) moved into "), "{line}");
        assert!(line.ends_with("Nothing was deleted."), "{line}");
    }

    #[test]
    fn side_files_a_cut_short_move_left_follow_their_database() {
        let data = test_folder("orphans");
        let archive_folder = data.join(FOLDER);
        fs::create_dir(&archive_folder).unwrap();
        fs::write(archive_folder.join("library.db"), b"library").unwrap();
        fs::write(data.join("library.db-wal"), b"newest").unwrap();
        // A side file with no database anywhere is nobody's to move.
        fs::write(data.join("midi.db-shm"), b"stray").unwrap();
        // They follow whatever `ready` says now: their database went when it was.
        let done = archive(&data, &[], &|_| Err("no".to_owned()), &mut quiet);
        assert_eq!(done.moved, [("library.db", 6)]);
        assert!(done.waits.is_empty() && done.failed.is_empty(), "{done:?}");
        assert_eq!(fs::read(archive_folder.join("library.db-wal")).unwrap(), b"newest");
        assert_eq!(names(&data), ["midi.db-shm", FOLDER]);
    }

    #[test]
    fn nothing_the_archive_holds_is_ever_replaced() {
        let data = test_folder("kept");
        let archive_folder = data.join(FOLDER);
        fs::create_dir(&archive_folder).unwrap();
        fs::write(archive_folder.join("library.db"), b"archived").unwrap();
        fs::write(archive_folder.join("midi.db-wal"), b"archived side").unwrap();
        // library.db is back in the data folder (put back from a backup, say).
        fs::write(data.join("library.db"), b"back again").unwrap();
        fs::write(data.join("midi.db"), b"midi").unwrap();
        fs::write(data.join("midi.db-wal"), b"another").unwrap();
        fs::write(data.join("midi.db-shm"), b"index").unwrap();
        let done = archive(&data, &[], READY, &mut quiet);
        assert_eq!(
            done.waits,
            [format!("library.db is in the data folder again, and {FOLDER} already holds one: both stay as they are")]
        );
        assert_eq!(fs::read(archive_folder.join("library.db")).unwrap(), b"archived");
        assert_eq!(fs::read(data.join("library.db")).unwrap(), b"back again");
        // midi.db itself goes, and the side file there's room for; the one that would replace another stays.
        assert_eq!(done.moved, [("midi.db", 9)]);
        assert_eq!(
            done.failed,
            [format!("{FOLDER} already has a midi.db-wal, so the one in the data folder stays where it is")]
        );
        assert_eq!(fs::read(archive_folder.join("midi.db-wal")).unwrap(), b"archived side");
        assert_eq!(fs::read(data.join("midi.db-wal")).unwrap(), b"another");
        assert!(archive_folder.join("midi.db-shm").is_file() && !data.join("midi.db-shm").exists());
    }

    #[test]
    fn one_left_in_the_old_folder_is_copied_checked_and_left_as_it_is() {
        let data = test_folder("copies");
        let old = test_folder("copies-old");
        let big: Vec<u8> = (0..3 * BUFFER + 17).map(|i| (i * 31 % 251) as u8).collect();
        fs::write(old.join("sounds.db"), &big).unwrap();
        fs::write(old.join("sounds.db-wal"), b"newest").unwrap();
        let long_ago = SystemTime::now() - Duration::from_secs(40 * 24 * 60 * 60);
        File::options().write(true).open(old.join("sounds.db")).unwrap().set_modified(long_ago).unwrap();
        // The data folder's own library.db is the newer of the two: it's moved, and the old folder's is left out.
        fs::write(old.join("library.db"), b"as it was at the move").unwrap();
        fs::write(data.join("library.db"), b"changed since").unwrap();
        let before = names(&old);

        let mut heard = Vec::new();
        let done = archive(&data, std::slice::from_ref(&old), READY, &mut |at| {
            heard.push((at.name.to_owned(), at.done, at.total))
        });
        assert!(done.waits.is_empty() && done.failed.is_empty(), "{done:?}");
        assert_eq!(done.moved, [("library.db", 13)]);
        let total = big.len() as u64 + 6;
        assert_eq!(done.copied, [Copied { name: "sounds.db", bytes: total, from: old.clone() }]);
        let archive_folder = data.join(FOLDER);
        assert_eq!(names(&archive_folder), [READ_ME, "library.db", "sounds.db", "sounds.db-wal"]);
        assert_eq!(fs::read(archive_folder.join("library.db")).unwrap(), b"changed since");
        assert_eq!(fs::read(archive_folder.join("sounds.db")).unwrap(), big);
        assert_eq!(fs::read(archive_folder.join("sounds.db-wal")).unwrap(), b"newest");
        // The copy has the file's own time, and the old folder is as it was.
        let time = |p: PathBuf| fs::metadata(p).unwrap().modified().unwrap();
        assert_eq!(time(archive_folder.join("sounds.db")), time(old.join("sounds.db")));
        assert_eq!(names(&old), before);
        assert_eq!(fs::read(old.join("sounds.db")).unwrap(), big);
        assert_eq!(fs::read(old.join("library.db")).unwrap(), b"as it was at the move");
        // It said how far it was, side file first, to the end.
        assert_eq!(heard.first(), Some(&("sounds.db".to_owned(), 6, total)));
        assert_eq!(heard.last(), Some(&("sounds.db".to_owned(), total, total)));
        assert!(heard.windows(2).all(|w| w[0].1 <= w[1].1), "never backwards");
        let read_me = fs::read_to_string(archive_folder.join(READ_ME)).unwrap();
        assert!(read_me.contains(&format!("sounds.db: copied here from {}", old.display())), "{read_me}");

        let line = card_line(&done, &data, std::slice::from_ref(&old)).unwrap();
        assert!(line.starts_with("library.db (13 bytes) moved into "), "{line}");
        assert!(line.contains(&format!(") copied there from {} and checked. ", old.display())), "{line}");
        assert!(line.contains("Nothing was deleted."), "{line}");
        assert!(
            line.ends_with(&format!(
                "Nothing in {} is waited for any more: the note in it says what to keep, and the rest can go.",
                old.display()
            )),
            "{line}"
        );
        // Once it's there, the old folder's isn't read again.
        assert_eq!(
            archive(&data, std::slice::from_ref(&old), READY, &mut |_| panic!("copied again")),
            Archived::default()
        );
    }

    #[test]
    fn a_copy_cut_short_goes_on_from_the_files_already_there() {
        let data = test_folder("resumes");
        let old = test_folder("resumes-old");
        fs::write(old.join("sounds.db"), b"sounds").unwrap();
        fs::write(old.join("sounds.db-wal"), b"newest").unwrap();
        fs::write(old.join("midi.db"), b"midi").unwrap();
        fs::write(old.join("midi.db-wal"), b"midi's newest").unwrap();
        let archive_folder = data.join(FOLDER);
        fs::create_dir(&archive_folder).unwrap();
        // sounds.db's side file was copied before the app stopped; midi.db's place has another file of that name.
        fs::copy(old.join("sounds.db-wal"), archive_folder.join("sounds.db-wal")).unwrap();
        let time = fs::metadata(old.join("sounds.db-wal")).unwrap().modified().unwrap();
        File::options().write(true).open(archive_folder.join("sounds.db-wal")).unwrap().set_modified(time).unwrap();
        fs::write(archive_folder.join("midi.db-wal"), b"something else").unwrap();
        // A half-written copy from another run isn't taken for anything.
        fs::write(archive_folder.join("sounds.db.part-1"), b"sou").unwrap();

        let done = archive(&data, std::slice::from_ref(&old), READY, &mut quiet);
        assert_eq!(done.copied, [Copied { name: "sounds.db", bytes: 12, from: old.clone() }]);
        assert_eq!(fs::read(archive_folder.join("sounds.db")).unwrap(), b"sounds");
        assert_eq!(done.failed.len(), 1, "{done:?}");
        assert!(done.failed[0].starts_with("midi.db couldn't be copied from "), "{done:?}");
        assert!(done.failed[0].contains(&format!("{FOLDER} already has another midi.db-wal")), "{done:?}");
        assert!(!archive_folder.join("midi.db").exists(), "never without all of it");
        assert_eq!(fs::read(archive_folder.join("midi.db-wal")).unwrap(), b"something else");
        assert!(archive_folder.join("sounds.db.part-1").exists(), "the sweep's to clear, not a copy's");
        // The old folder still has one that isn't put away: it isn't called done with.
        let line = card_line(&done, &data, std::slice::from_ref(&old)).unwrap();
        assert!(line.starts_with("sounds.db (12 bytes) copied into "), "{line}");
        assert!(line.ends_with("Nothing was deleted."), "{line}");
    }

    #[test]
    fn one_in_the_old_folder_waits_for_its_own_replacement_too() {
        let data = test_folder("old-waits");
        let old = test_folder("old-waits-old");
        fs::write(old.join("sounds.db"), b"sounds").unwrap();
        let done =
            archive(&data, std::slice::from_ref(&old), &|_| Err("not yet".to_owned()), &mut |_| panic!("copied"));
        assert_eq!(done.waits, [format!("sounds.db stays in {} for now: not yet", old.display())]);
        assert!(!data.join(FOLDER).exists());
        // An old folder that isn't there (its drive is away) is nothing to do.
        assert_eq!(archive(&data, &[old.join("gone")], READY, &mut quiet), Archived::default());
    }

    #[test]
    fn one_left_two_moves_back_is_found_through_each_folders_own_record() {
        let root = test_folder("two-back");
        let (a, b, c) = (root.join("A"), root.join("B"), root.join("C"));
        for (folder, from) in [(&a, None), (&b, Some(&a)), (&c, Some(&b))] {
            fs::create_dir(folder).unwrap();
            if let Some(from) = from {
                let record = crate::data_move::Record { from: from.display().to_string(), ..Default::default() };
                fs::write(folder.join(crate::paths::MOVED_IN_FILE), serde_json::to_string(&record).unwrap()).unwrap();
            }
        }
        assert_eq!(old_folders(&c), [b.clone(), a.clone()]);
        assert_eq!(old_folders(&a), Vec::<PathBuf>::new());
        // sounds.db stayed in A at the first move; B, the folder before this one, has an older midi.db than A's.
        fs::write(a.join("sounds.db"), b"sounds").unwrap();
        fs::write(a.join("midi.db"), b"older").unwrap();
        fs::write(b.join("midi.db"), b"newer!").unwrap();
        let done = archive(&c, &old_folders(&c), READY, &mut quiet);
        assert!(done.failed.is_empty() && done.waits.is_empty(), "{done:?}");
        assert_eq!(
            done.copied,
            [
                Copied { name: "sounds.db", bytes: 6, from: a.clone() },
                Copied { name: "midi.db", bytes: 6, from: b.clone() }
            ]
        );
        assert_eq!(fs::read(c.join(FOLDER).join("midi.db")).unwrap(), b"newer!");
        let line = card_line(&done, &c, &old_folders(&c)).unwrap();
        assert!(
            line.contains(&format!("midi.db (6 bytes) copied into {} from {}", c.join(FOLDER).display(), b.display())),
            "{line}"
        );
        assert!(line.contains(&format!("sounds.db (6 bytes) copied there from {}", a.display())), "{line}");
        assert!(
            line.contains(&format!("Nothing in {} and {} is waited for any more", b.display(), a.display())),
            "{line}"
        );
        // A record that points back at itself ends the walk.
        let record = crate::data_move::Record { from: c.display().to_string(), ..Default::default() };
        fs::write(a.join(crate::paths::MOVED_IN_FILE), serde_json::to_string(&record).unwrap()).unwrap();
        assert_eq!(old_folders(&c), [b, a]);
    }

    #[cfg(unix)]
    #[test]
    fn nothing_is_put_away_through_a_link() {
        let data = test_folder("link");
        let elsewhere = test_folder("link-elsewhere");
        std::os::unix::fs::symlink(&elsewhere, data.join(FOLDER)).unwrap();
        fs::write(data.join("library.db"), b"library").unwrap();
        // A database that is itself a link is never moved either.
        fs::write(elsewhere.join("real-midi.db"), b"midi").unwrap();
        std::os::unix::fs::symlink(elsewhere.join("real-midi.db"), data.join("midi.db")).unwrap();
        let done = archive(&data, &[], READY, &mut quiet);
        assert!(!done.any(), "{done:?}");
        assert_eq!(done.waits.len(), 1, "{done:?}");
        assert!(done.waits[0].ends_with("isn't a plain folder"), "{done:?}");
        assert!(data.join("library.db").is_file());
        assert_eq!(names(&elsewhere), ["real-midi.db"]);
    }

    #[test]
    fn a_half_written_copy_is_known_by_its_whole_name() {
        for name in ["sounds.db.part-1234", "library.db-wal.part-7", "midi.db-shm.part-22", "midi.db-journal.part-1"] {
            assert!(is_part(name), "{name}");
        }
        for name in [
            "sounds.db",
            "sounds.db.part-",
            "sounds.db.part-12a",
            "sounds.db.part-1.part-x",
            "songs.db.part-12",
            "sounds.db-old.part-12",
            "my sounds.db.part-12",
            READ_ME,
        ] {
            assert!(!is_part(name), "{name}");
        }
        assert_eq!(mo_space::app_files::part_of(FOLDER, true).0, mo_space::app_files::OLD_DATABASES);
    }

    #[test]
    fn a_week_is_counted_from_when_the_apps_own_took_over() {
        let day = 24 * 60 * 60;
        let now = 1_800_000_000;
        assert_eq!(worked(now - 7 * day, now, "x"), Ok(()));
        assert_eq!(
            worked(now - 7 * day + 1, now, "x has been y"),
            Err("x has been y for 6 days: it's put away after 7".into())
        );
        assert_eq!(worked(now - day - 5, now, "x"), Err("x for 1 day: it's put away after 7".into()));
        assert_eq!(worked(now, now, "x"), Err("x for 0 days: it's put away after 7".into()));
        assert!(worked(now + 60, now, "x").unwrap_err().contains("after now"));
    }

    /// A C# `library.db` with one FL Studio save, and the app's own `songs.db` made from it and taken as its own.
    fn own_song_library(data: &Path) {
        rusqlite::Connection::open(data.join("library.db"))
            .unwrap()
            .execute_batch(
                "CREATE TABLE projects (id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL COLLATE NOCASE,
                     name TEXT NOT NULL, daw TEXT NOT NULL, info TEXT);
                 INSERT INTO projects VALUES (1, 'C:\\Music\\a.flp', 'a', 'FL Studio', '{}');",
            )
            .unwrap();
        let songs = data.join("songs.db");
        mo_library::songs_db::make(&data.join("library.db"), &songs, &mut |_| {}).unwrap();
        assert!(mo_library::songs_db::at_start(data, "test").is_songs_db());
        assert!(mo_library::songs_db::own_since(&songs).is_some());
    }

    #[test]
    fn library_db_goes_a_week_after_songs_db_took_over_once_the_mend_has_read_it() {
        let data = test_folder("gate-library");
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
        let week = WORKED_FOR.as_secs() as i64;
        assert_eq!(
            ready_at(&data, "library.db", now + week),
            Err("songs.db isn't the app's own song library yet".into())
        );
        own_song_library(&data);
        // Read the clock again: the library's own time may be a second after the first reading.
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
        let why = ready_at(&data, "library.db", now).unwrap_err();
        assert!(why.starts_with("songs.db has been the app's own song library for 0 days"), "{why}");
        let later = now + week + 60;
        let why = ready_at(&data, "library.db", later).unwrap_err();
        assert!(why.contains("muted samples"), "{why}");
        // The whole step: nothing moves until the mend is noted.
        let feed = Feed::default();
        let done = archive(&data, &[], &|old| ready_at(&data, old, later), &mut quiet);
        assert!(!done.any() && done.waits.len() == 1, "{done:?}");
        let batch = mo_library::song_writes::Batch { mended: true, ..Default::default() };
        mo_library::song_writes::write(&data.join("songs.db"), &batch).unwrap();
        assert_eq!(ready_at(&data, "library.db", later), Ok(()));
        let done = archive(&data, &[], &|old| ready_at(&data, old, later), &mut quiet);
        assert_eq!(done.moved.len(), 1, "{done:?}");
        assert!(data.join(FOLDER).join("library.db").is_file() && !data.join("library.db").exists());
        // The app opens its own library as before, with library.db gone.
        assert!(mo_library::songs_db::at_start(&data, "test").is_songs_db());
        assert_eq!(mo_library::song_writes::to_mend(&data.join("songs.db")).unwrap(), None);
        // Run now, as the app does: nothing is left to do, and no card for nothing.
        assert_eq!(archive_now(&data, &[], &data, &feed), Archived::default());
        assert!(feed.visible(std::time::Instant::now()).is_empty());
    }

    #[test]
    fn midi_db_goes_a_week_after_the_index_took_its_corrections_in() {
        let data = test_folder("gate-midi");
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
        let week = WORKED_FOR.as_secs() as i64;
        let why = ready_at(&data, "midi.db", now + week).unwrap_err();
        assert!(why.starts_with("the MIDI index hasn't taken in the old app's corrections yet"), "{why}");
        let index = data.join(mo_midi::store::FILE_NAME);
        drop(mo_midi::store::MidiStore::open(&index).unwrap());
        assert!(ready_at(&data, "midi.db", now + week).is_err(), "an index alone has taken nothing in");
        rusqlite::Connection::open(&index)
            .unwrap()
            .execute("INSERT INTO about (name, value) VALUES ('took_in_old_corrections', ?1)", [now])
            .unwrap();
        let why = ready_at(&data, "midi.db", now + 3 * 24 * 60 * 60).unwrap_err();
        assert_eq!(why, "the MIDI index has had the old app's corrections for 3 days: it's put away after 7");
        assert_eq!(ready_at(&data, "midi.db", now + week), Ok(()));
        assert!(ready_at(&data, "curves.db", now + week).is_err());
    }

    #[test]
    fn sounds_db_stays_until_the_apps_own_sound_library_is() {
        let data = test_folder("gate-sounds");
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
        assert_eq!(
            ready_at(&data, "sounds.db", now + WORKED_FOR.as_secs() as i64),
            Err("sounds-rust.db isn't the app's own sound library yet".into())
        );
    }

    #[test]
    fn the_card_names_what_went_where() {
        let data = PathBuf::from("data");
        let done = Archived {
            moved: vec![("library.db", 3 << 20), ("midi.db", 1 << 20)],
            copied: vec![Copied { name: "sounds.db", bytes: 5 << 20, from: "gone-old-folder".into() }],
            ..Archived::default()
        };
        let line = card_line(&done, &data, &["gone-old-folder".into()]).unwrap();
        let archive_folder = data.join(FOLDER);
        assert_eq!(
            line,
            format!(
                "library.db and midi.db (4 MB) moved into {}. sounds.db (5 MB) copied there from \
                 gone-old-folder and checked. The app has its own now and doesn't open them any more. Nothing was \
                 deleted.",
                archive_folder.display()
            )
        );
        assert_eq!(and(&["a", "b", "c"]), "a, b and c");
    }
}
