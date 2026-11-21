//! Reading a file back from the drive itself (past the system's memory of it), on files made in the test build's own
//! fixtures folder under fixed names, never a random temp one and never a real folder.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::io::{BufRead, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

use mo_backup::from_drive::FromDrive;

fn fixtures(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::canonicalize(&dir).unwrap()
}

/// Bytes that change all the way along, so a piece read from the wrong place shows.
fn bytes(n: usize) -> Vec<u8> {
    let mut x: u32 = 0x9E37_79B9;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x.to_le_bytes()[0]
        })
        .collect()
}

/// Windows, Linux and a Mac all read past their memory of a file on an ordinary drive.
const PAST_MEMORY: bool = cfg!(any(windows, target_os = "linux", target_vendor = "apple"));

const MIB: usize = 1 << 20;

/// What this program has fetched from storage so far, as Linux counts it (`/proc/self/io`): nothing on other systems.
fn fetched() -> Option<u64> {
    let io = fs::read_to_string("/proc/self/io").ok()?;
    io.lines().find_map(|l| l.strip_prefix("read_bytes:"))?.trim().parse().ok()
}

/// A file just written is fetched from the drive again, not read from the system's memory of it: where Linux counts
/// what this program fetched from storage, the count grows by at least the file's size (an ordinary read of it fetches
/// nothing, as it's all still in memory).
#[test]
fn a_file_just_written_is_fetched_from_the_drive() {
    let dir = fixtures("from-drive-fetched");
    let path = dir.join("copy.bin");
    let want = bytes(8 * MIB);
    let mut out = fs::File::create(&path).unwrap();
    out.write_all(&want).unwrap();
    out.sync_all().unwrap();
    drop(out);
    let before = fetched();
    let mut file = FromDrive::open(&path).unwrap();
    let mut got = Vec::new();
    file.read_to_end(&mut got).unwrap();
    let after = fetched();
    assert!(got == want, "read back {} bytes, not the same", got.len());
    assert_eq!(file.past_memory(), PAST_MEMORY);
    if let (Some(before), Some(after)) = (before, after) {
        let from_drive = after - before;
        assert!(from_drive >= want.len() as u64, "fetched {from_drive} bytes from the drive, not {}", want.len());
    }
}

#[test]
fn a_file_reads_back_whole_from_the_drive() {
    let dir = fixtures("from-drive-whole");
    // Empty, smaller than a sector, around the 64 KiB the reads line up on, a whole block, and blocks and a bit.
    for size in [0, 1, 100, 65_535, 65_536, 65_537, MIB - 1, MIB, MIB + 1, 3 * MIB + 12_345] {
        let path = dir.join(format!("{size}.bin"));
        let want = bytes(size);
        fs::write(&path, &want).unwrap();

        // Piece by piece, as a copy's check reads it.
        let mut file = FromDrive::open(&path).unwrap();
        let mut got = Vec::new();
        loop {
            let piece = file.fill_buf().unwrap();
            if piece.is_empty() {
                break;
            }
            assert!(piece.len() <= MIB, "{size}: one block at most");
            got.extend_from_slice(piece);
            let n = piece.len();
            file.consume(n);
        }
        assert!(got == want, "{size}: read back {} bytes, not the same", got.len());
        // Read to the end without falling back to the ordinary way: on Windows, every read lined up on its sectors.
        assert_eq!(file.past_memory(), PAST_MEMORY, "{size}");

        // In small reads, as a zip reads it.
        let mut file = FromDrive::open(&path).unwrap();
        let mut got = Vec::new();
        let mut buf = [0u8; 1000];
        loop {
            let n = file.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            got.extend_from_slice(&buf[..n]);
        }
        assert!(got == want, "{size}: read back {} bytes in small reads, not the same", got.len());
        assert_eq!(file.past_memory(), PAST_MEMORY, "{size}");
    }
}

/// A file that grows while it's read (a copy just written and closed shouldn't) is read as long as it was when opened.
#[test]
fn a_file_reads_back_as_long_as_it_was_when_opened() {
    let dir = fixtures("from-drive-grown");
    let path = dir.join("copy.bin");
    let want = bytes(100_000);
    fs::write(&path, &want).unwrap();
    let mut file = FromDrive::open(&path).unwrap();
    fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(&[7u8; 5000]).unwrap();
    let mut got = Vec::new();
    file.read_to_end(&mut got).unwrap();
    assert!(got == want, "read back {} bytes, not the {} it had", got.len(), want.len());
}

#[test]
fn any_part_of_a_file_reads_back_from_the_drive() {
    let dir = fixtures("from-drive-anywhere");
    let size = 3 * MIB + 12_345;
    let want = bytes(size);
    let path = dir.join("song.zip");
    fs::write(&path, &want).unwrap();
    let mut file = FromDrive::open(&path).unwrap();

    let read = |file: &mut FromDrive, at: u64, n: usize| {
        assert_eq!(file.seek(SeekFrom::Start(at)).unwrap(), at);
        let mut got = vec![0u8; n];
        file.read_exact(&mut got).unwrap();
        let at = usize::try_from(at).unwrap();
        assert!(got == want[at..at + n], "{n} bytes from {at}");
    };
    // Across the block's end and the 64 KiB lines, backwards, inside one block, and the file's last bytes.
    read(&mut file, (MIB - 10) as u64, 20);
    read(&mut file, (2 * MIB + 65_530) as u64, 70_000);
    read(&mut file, 5, 3);
    read(&mut file, 65_536, 1);
    read(&mut file, (MIB + 100) as u64, MIB + 1000);
    read(&mut file, (size - 1) as u64, 1);
    read(&mut file, 0, size);

    // From the end and from here, as a zip finds its list of files.
    assert_eq!(file.seek(SeekFrom::End(-7)).unwrap(), (size - 7) as u64);
    let mut end = Vec::new();
    file.read_to_end(&mut end).unwrap();
    assert!(end == want[size - 7..]);
    assert_eq!(file.seek(SeekFrom::Current(-3)).unwrap(), (size - 3) as u64);
    let mut last = [0u8; 3];
    file.read_exact(&mut last).unwrap();
    assert_eq!(last, want[size - 3..]);
    assert_eq!(file.stream_position().unwrap(), size as u64);

    // Past the end there's nothing; before the start is no place.
    assert_eq!(file.seek(SeekFrom::Start(size as u64 + 10)).unwrap(), size as u64 + 10);
    assert_eq!(file.read(&mut [0u8; 10]).unwrap(), 0);
    assert!(file.seek(SeekFrom::End(-(size as i64) - 1)).is_err());
    assert!(file.seek(SeekFrom::Current(-(size as i64) - 11)).is_err());
    assert_eq!(file.stream_position().unwrap(), size as u64 + 10, "a seek that fails stays where it was");
    assert_eq!(file.past_memory(), PAST_MEMORY);
}
