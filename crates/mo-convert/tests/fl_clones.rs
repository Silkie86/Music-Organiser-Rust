//! Lining up cloned patterns in an FL project, on made-up saves built byte by byte (none of Solomon's files are in the
//! repo).

// Test files may unwrap: a test that stops there has failed, which is what it should do.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use mo_convert::fl_clones::{Group, Move, fix_fl_project, line_up};

/// An FL project's events, then the file around them.
#[derive(Default)]
struct Flp(Vec<u8>);

impl Flp {
    fn word(mut self, id: u8, v: u16) -> Flp {
        self.0.push(id);
        self.0.extend(v.to_le_bytes());
        self
    }

    fn data(mut self, id: u8, data: &[u8]) -> Flp {
        self.0.push(id);
        let mut n = data.len();
        loop {
            let b = (n & 0x7F) as u8;
            n >>= 7;
            if n == 0 {
                self.0.push(b);
                break;
            }
            self.0.push(b | 0x80);
        }
        self.0.extend(data);
        self
    }

    fn version(self, v: &str) -> Flp {
        let mut b = v.as_bytes().to_vec();
        b.push(0);
        self.data(199, &b)
    }

    /// A pattern, its name (UTF-16, as FL 11.5 and later write it) and one note on each channel given.
    fn pattern(self, number: u16, name: &str, channels: &[u16]) -> Flp {
        let mut flp = self.word(65, number);
        if !name.is_empty() {
            let text: Vec<u8> = name.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect();
            flp = flp.data(193, &text);
        }
        let mut notes = Vec::new();
        for &c in channels {
            let mut n = [0u8; 24];
            n[6..8].copy_from_slice(&c.to_le_bytes());
            n[8..12].copy_from_slice(&96u32.to_le_bytes());
            n[12..14].copy_from_slice(&60u16.to_le_bytes());
            notes.extend(n);
        }
        if notes.is_empty() { flp } else { flp.data(224, &notes) }
    }

    fn build(&self) -> Vec<u8> {
        let mut b = b"FLhd".to_vec();
        b.extend(6i32.to_le_bytes());
        b.extend(0i16.to_le_bytes());
        b.extend(3u16.to_le_bytes());
        b.extend(96u16.to_le_bytes());
        b.extend(b"FLdt");
        b.extend((self.0.len() as i32).to_le_bytes());
        b.extend(&self.0);
        b
    }
}

/// A 32-byte playlist clip on track `track` of 500: a pattern's (`pattern` > 0) or a channel's.
fn clip(at: u32, length: u32, pattern: u16, channel: u16, track: u16) -> [u8; 32] {
    let mut r = [0u8; 32];
    r[..4].copy_from_slice(&at.to_le_bytes());
    r[4..6].copy_from_slice(&20480u16.to_le_bytes());
    let index = if pattern > 0 { 20480 + pattern } else { channel };
    r[6..8].copy_from_slice(&index.to_le_bytes());
    r[8..12].copy_from_slice(&length.to_le_bytes());
    r[12..14].copy_from_slice(&(500 - track).to_le_bytes());
    r[24..32].copy_from_slice(&[0xFF; 8]);
    r
}

/// A song re-saved by FL 20 from an FL 8 one: every pattern on its own track.
fn song() -> Flp {
    Flp::default()
        .version("20.0.5.681")
        .pattern(1, "bass", &[3])
        .pattern(2, "bass #3", &[3])
        .pattern(3, "bass #2", &[3, 4])
        .pattern(4, "", &[0, 1])
        .pattern(5, "Pattern 4 #2", &[0])
        .pattern(6, "", &[7])
        .pattern(7, "Pattern 1 #2", &[8])
        .pattern(8, "lead", &[5])
        .pattern(9, "lead #2", &[5])
        .pattern(12, "hats #2", &[9])
        .pattern(13, "hats", &[10])
        .pattern(10, "chords", &[2])
        .pattern(11, "chords main", &[2])
}

fn playlist(clips: &[[u8; 32]]) -> Vec<u8> {
    clips.concat()
}

#[test]
fn clones_go_on_their_sources_track() {
    let clips = [
        clip(0, 384, 1, 0, 1),
        clip(384, 384, 2, 0, 2),
        clip(768, 384, 3, 0, 3),
        clip(0, 384, 4, 0, 4),
        clip(384, 384, 5, 0, 5),
        clip(0, 384, 6, 0, 6),
        clip(384, 384, 7, 0, 7),
        clip(0, 384, 8, 0, 8),
        clip(0, 384, 9, 0, 9),
        clip(0, 384, 10, 0, 10),
        clip(384, 384, 11, 0, 11),
    ];
    let raw = song().data(233, &playlist(&clips)).build();
    let lineup = line_up(&raw).unwrap();

    assert_eq!(
        lineup.groups,
        vec![
            Group { source: 1, clones: vec![2, 3] },
            Group { source: 4, clones: vec![5] },
            Group { source: 8, clones: vec![9] }
        ]
    );
    let moved: Vec<(u32, u32, u32)> = lineup.moves.iter().map(|m| (m.pattern, m.from_track, m.to_track)).collect();
    // "lead #2" plays alongside "lead", so it stays.
    assert_eq!(moved, vec![(2, 2, 1), (5, 5, 4), (3, 3, 1)]);
    let left = lineup.left.join("\n");
    assert!(left.contains("lead #2 at tick 0 stays on track 9"), "{left}");
    assert!(left.contains("Pattern 1 #2 stays: the unnamed pattern before it (Pattern 6) has none"), "{left}");
    assert!(left.contains("hats #2 stays: it has none of hats's channels"), "{left}");
    assert!(!left.contains("chords"), "patterns without a # name aren't clones: {left}");
    assert_eq!(lineup.summary()[0], "bass #3: 1 clip from track 2 to track 1");

    // Only the moved clips' tracks change.
    let fixed = lineup.apply(&raw);
    assert_eq!(fixed.len(), raw.len());
    let changed: Vec<usize> = (0..raw.len()).filter(|&i| raw[i] != fixed[i]).collect();
    assert_eq!(changed.len(), 3, "one byte of each moved clip's track: {changed:?}");
    assert!(line_up(&fixed).unwrap().moves.is_empty());
}

#[test]
fn a_clone_never_lands_on_a_clip_already_on_the_track() {
    // An audio clip on the bass's track plays where "bass #2" would go.
    let clips = [clip(0, 384, 1, 0, 1), clip(384, 384, 0, 12, 1), clip(384, 384, 3, 0, 3), clip(768, 96, 3, 0, 3)];
    let raw = song().data(233, &playlist(&clips)).build();
    let lineup = line_up(&raw).unwrap();
    assert_eq!(lineup.moves, vec![Move { arrangement: 0, pattern: 3, at: 768, from_track: 3, to_track: 1 }]);
    assert!(lineup.left[lineup.left.len() - 1].starts_with("bass #2 at tick 384 stays on track 3"));
}

#[test]
fn each_arrangement_is_lined_up_on_its_own() {
    let first = playlist(&[clip(0, 384, 1, 0, 1), clip(384, 384, 3, 0, 3)]);
    // In the second arrangement the bass sits on track 5.
    let second = playlist(&[clip(0, 384, 1, 0, 5), clip(384, 384, 3, 0, 2)]);
    let raw = song().word(99, 0).data(233, &first).word(99, 1).data(233, &second).build();
    let moved: Vec<(usize, u32, u32)> =
        line_up(&raw).unwrap().moves.iter().map(|m| (m.arrangement, m.from_track, m.to_track)).collect();
    assert_eq!(moved, vec![(0, 3, 1), (1, 2, 5)]);
}

#[test]
fn an_old_save_with_a_row_per_pattern_is_left_alone() {
    // FL 9: 32-byte records with 0 and 0x6000 where the pattern goes, on row 999 − pattern.
    let mut rows = Vec::new();
    for p in [1u16, 3] {
        let mut r = [0u8; 32];
        r[6..8].copy_from_slice(&0x6000u16.to_le_bytes());
        r[8..12].copy_from_slice(&384u32.to_le_bytes());
        r[12..14].copy_from_slice(&(999 - p).to_le_bytes());
        rows.extend(r);
    }
    let raw = Flp::default().version("9.0.0").pattern(1, "bass", &[3]).pattern(3, "bass #2", &[3]).data(233, &rows);
    let lineup = line_up(&raw.build()).unwrap();
    assert!(lineup.rows);
    assert!(lineup.moves.is_empty());
}

#[test]
fn the_fix_writes_a_copy_and_never_changes_the_project() {
    let dir = std::env::temp_dir().join(format!("mo-convert-test-fl-clones-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let project = dir.join("old song_3.flp");
    let raw = song().data(233, &playlist(&[clip(0, 384, 1, 0, 1), clip(384, 384, 3, 0, 3)])).build();
    std::fs::write(&project, &raw).unwrap();

    let fixed = fix_fl_project(&project).unwrap();
    assert_eq!(fixed.file, dir.join("old song_3 (fixed).flp"));
    assert_eq!(std::fs::read(&project).unwrap(), raw, "the project itself isn't changed");
    assert!(line_up(&std::fs::read(&fixed.file).unwrap()).unwrap().moves.is_empty());
    // Again: a second copy, never over the first.
    assert_eq!(fix_fl_project(&project).unwrap().file, dir.join("old song_3 (fixed 2).flp"));
    // A fixed copy has nothing left to move.
    let again = fix_fl_project(&fixed.file).unwrap_err();
    assert!(again.contains("already on their sources' tracks"), "{again}");
    let _ = std::fs::remove_dir_all(&dir);
}
