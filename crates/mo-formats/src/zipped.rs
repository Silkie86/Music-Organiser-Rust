//! Taking a file out of a project's zip without trusting the size the zip gives it.

use std::io::Read;

/// The most a file in a project's zip may hold: far above any song's XML or plugin state. A damaged or made-up zip
/// can say a small file unpacks to gigabytes, and the app would close when it ran out of memory.
pub const MOST: u64 = 256 * 1024 * 1024;

/// A file's bytes from the zip, or none when they don't come out whole or come to more than [`MOST`].
pub fn read_whole(entry: impl Read) -> Option<Vec<u8>> {
    let mut data = Vec::new();
    entry.take(MOST + 1).read_to_end(&mut data).ok()?;
    (data.len() as u64 <= MOST).then_some(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_bigger_than_the_most_is_left_out() {
        assert_eq!(read_whole(&b"state"[..]).as_deref(), Some(&b"state"[..]));
        assert!(read_whole(std::io::repeat(0)).is_none(), "it stops just past the most");
    }
}
