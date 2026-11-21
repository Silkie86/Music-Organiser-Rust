//! Reading a file back from the drive itself, not from the system's memory of it.
//!
//! A file just written stays in the system's memory (its cache) for a while, and an ordinary read is answered from
//! there: reading a copy back that way only proves the system got the bytes, not that the drive keeps them. A backup
//! drive behind a bad USB lead or bridge can spoil what it's sent without a word, so Backup reads each new copy back
//! through [`FromDrive`] (the whole-app review of 19 Nov, SB-2; Solomon picked reading back from the drive):
//! - **Windows** reads the file with its memory of it turned off (`FILE_FLAG_NO_BUFFERING`). Each read then has to
//!   start a whole number of sectors into the file, be a whole number of sectors long, and land in memory that starts
//!   on a sector, so the bytes come in through a block lined up on [`ALIGN`] bytes, which is a whole number of sectors
//!   on every drive.
//! - **Linux** is told first to let go of what it holds of the file (`posix_fadvise`, "don't need"). The copy was
//!   flushed to the drive before, so all of it can go, and the reads find the drive's copy.
//! - **A Mac** reads with its memory turned off for the file (`F_NOCACHE`). That only keeps what's read now out of its
//!   memory: what it still holds from the writing may answer instead, so on a Mac the check is weaker.
//!
//! The drive's own small memory can still answer, but by then the bytes have come back over the lead and through the
//! bridge, where a backup drive most often goes wrong. When the system won't read past its memory (a drive or network
//! share that refuses), the file is read the ordinary way, and [`FromDrive::past_memory`] says so.

use std::fs::File;
use std::io::{self, BufRead, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// What every read lines up on, in the file and in memory: 64 KiB, a whole number of sectors on any drive (512 bytes
/// or 4 KiB).
const ALIGN: usize = 64 * 1024;

/// The most read at once: 1 MiB, as copies are written.
const BLOCK: usize = crate::copy::BUFFER;

/// A file read from the drive itself (see the module notes), piece by piece through a block of memory ([`BufRead`]),
/// or in any order ([`Seek`]), as a zip is read.
pub struct FromDrive {
    path: PathBuf,
    file: File,
    past_memory: bool,
    /// The memory the reads land in: one [`ALIGN`] more than the block, so the block can start on a lined-up place.
    room: Vec<u8>,
    /// Where the block starts in `room`.
    start: usize,
    /// How long the block is: the file's size up to [`BLOCK`], in whole [`ALIGN`]s.
    block: usize,
    /// Where in the file the block's bytes come from (a whole number of [`ALIGN`]s while reading past the memory), and
    /// how many it holds.
    at: u64,
    filled: usize,
    /// Where the next read starts.
    pos: u64,
    /// The file's size when it was opened: what's read.
    size: u64,
}

impl FromDrive {
    /// Opens the file to be read from the drive (see the module notes).
    pub fn open(path: &Path) -> io::Result<FromDrive> {
        let (file, past_memory) = open_past_memory(path)?;
        let size = file.metadata()?.len();
        let block = usize::try_from(size).unwrap_or(BLOCK).clamp(1, BLOCK).next_multiple_of(ALIGN);
        let (room, start) = room_for(block);
        Ok(FromDrive {
            path: path.to_path_buf(),
            file,
            past_memory,
            room,
            start,
            block,
            at: 0,
            filled: 0,
            pos: 0,
            size,
        })
    }

    /// Whether the system was asked to read past its memory of the file, and has read it so up to now (false when it
    /// refused, and the file is read the ordinary way).
    pub fn past_memory(&self) -> bool {
        self.past_memory
    }

    /// Reads the block of the file that holds the next byte: from the whole number of [`ALIGN`]s at or before it while
    /// reading past the memory (an ordinary read can start anywhere), to the file's size at most.
    fn load(&mut self) -> io::Result<()> {
        let line = if self.past_memory { ALIGN as u64 } else { 1 };
        self.at = self.pos - self.pos % line;
        self.filled = 0;
        while self.filled < self.block {
            let into = &mut self.room[self.start + self.filled..self.start + self.block];
            match read_at(&self.file, into, self.at + self.filled as u64) {
                Ok(0) => break,
                Ok(n) => {
                    self.filled += n;
                    // Less than a whole number of ALIGNs: the end of the file (a read from there wouldn't line up).
                    if n % ALIGN != 0 {
                        break;
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                // A drive whose sectors don't line up after all ("The parameter is incorrect", Windows says).
                Err(e) if self.past_memory && refused(&e) => return self.ordinary(),
                Err(e) => {
                    self.filled = 0;
                    return Err(e);
                }
            }
        }
        // Should the file have grown since it was opened, what's past its size then isn't read.
        self.filled = self.filled.min(usize::try_from(self.size - self.at).unwrap_or(usize::MAX));
        // The drive gave less than it was asked for before the end, and nothing from the next byte on (a network share
        // can): a lined-up read can't start there, an ordinary one can.
        if self.past_memory && self.at + self.filled as u64 <= self.pos {
            return self.ordinary();
        }
        Ok(())
    }

    /// Reads on from the next byte the ordinary way.
    fn ordinary(&mut self) -> io::Result<()> {
        self.file = File::open(&self.path)?;
        self.past_memory = false;
        self.load()
    }
}

impl BufRead for FromDrive {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        if self.pos >= self.size {
            return Ok(&[]);
        }
        if self.pos < self.at || self.pos >= self.at + self.filled as u64 {
            self.load()?;
        }
        // Past what the block holds only when the file is shorter than it was: its end.
        let from = usize::try_from(self.pos - self.at).map_or(self.filled, |from| from.min(self.filled));
        Ok(&self.room[self.start + from..self.start + self.filled])
    }

    fn consume(&mut self, n: usize) {
        self.pos += n as u64;
    }
}

impl Read for FromDrive {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let have = self.fill_buf()?;
        let n = have.len().min(buf.len());
        buf[..n].copy_from_slice(&have[..n]);
        self.consume(n);
        Ok(n)
    }
}

impl Seek for FromDrive {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let pos = match to {
            SeekFrom::Start(n) => Some(n),
            SeekFrom::End(n) => self.size.checked_add_signed(n),
            SeekFrom::Current(n) => self.pos.checked_add_signed(n),
        };
        self.pos = pos.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "A place before the file's start"))?;
        Ok(self.pos)
    }
}

/// Memory for a block of `block` bytes that starts on a lined-up place ([`ALIGN`]): the memory, and where in it the
/// block starts.
fn room_for(block: usize) -> (Vec<u8>, usize) {
    let room = vec![0u8; block + ALIGN];
    let start = (ALIGN - room.as_ptr().addr() % ALIGN) % ALIGN;
    (room, start)
}

/// Whether the system refused to read without its memory of the file (the drive or share can't, or its sectors don't
/// line up after all), rather than the file being gone, locked or unreadable. Windows says "Incorrect function", "The
/// request is not supported", "The parameter is incorrect" or "This function is not supported on this system".
fn refused(e: &io::Error) -> bool {
    let windows_refused = cfg!(windows) && matches!(e.raw_os_error(), Some(1 | 50 | 87 | 120));
    windows_refused || matches!(e.kind(), io::ErrorKind::InvalidInput | io::ErrorKind::Unsupported)
}

/// Opens the file with Windows' memory of it turned off. Gives whether it was.
#[cfg(windows)]
fn open_past_memory(path: &Path) -> io::Result<(File, bool)> {
    use std::os::windows::fs::OpenOptionsExt;
    /// Windows' flag for reading and writing a file without its memory of it (winbase.h).
    const FILE_FLAG_NO_BUFFERING: u32 = 0x2000_0000;
    match std::fs::OpenOptions::new().read(true).custom_flags(FILE_FLAG_NO_BUFFERING).open(path) {
        Ok(file) => Ok((file, true)),
        // A drive or share that won't: the ordinary way. Anything else (gone, locked) is said as it is.
        Err(e) if refused(&e) => Ok((File::open(path)?, false)),
        Err(e) => Err(e),
    }
}

/// Opens the file and tells Linux to let go of what it holds of it. Gives whether it would.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn open_past_memory(path: &Path) -> io::Result<(File, bool)> {
    let file = File::open(path)?;
    let let_go = rustix::fs::fadvise(&file, 0, None, rustix::fs::Advice::DontNeed).is_ok();
    Ok((file, let_go))
}

/// Opens the file with the Mac's memory turned off for it. Gives whether it was.
#[cfg(target_vendor = "apple")]
fn open_past_memory(path: &Path) -> io::Result<(File, bool)> {
    let file = File::open(path)?;
    let off = rustix::fs::fcntl_nocache(&file, true).is_ok();
    Ok((file, off))
}

/// Any other system: the ordinary way.
#[cfg(not(any(windows, target_os = "linux", target_os = "android", target_vendor = "apple")))]
fn open_past_memory(path: &Path) -> io::Result<(File, bool)> {
    Ok((File::open(path)?, false))
}

#[cfg(windows)]
fn read_at(file: &File, buf: &mut [u8], at: u64) -> io::Result<usize> {
    std::os::windows::fs::FileExt::seek_read(file, buf, at)
}

#[cfg(unix)]
fn read_at(file: &File, buf: &mut [u8], at: u64) -> io::Result<usize> {
    std::os::unix::fs::FileExt::read_at(file, buf, at)
}

#[cfg(test)]
mod tests {
    use super::{ALIGN, BLOCK, room_for};

    /// Windows reads past its memory only into memory that starts on a sector: the block always does, and fits.
    #[test]
    fn the_block_starts_on_a_lined_up_place() {
        for block in [ALIGN, 3 * ALIGN, BLOCK] {
            let (room, start) = room_for(block);
            assert_eq!(room[start..].as_ptr().addr() % ALIGN, 0, "{block}");
            assert!(start + block <= room.len(), "{block}");
        }
    }
}
