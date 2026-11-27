//! The stage tuner: measures the newest save of each of your songs (as the Songs board does) and tries the lines
//! between the stages ([`mo_library::stage::Limits`]) against the statuses you've given your songs.
//!
//! `stagetune OUT [SONGS_DB]` reads the app's songs.db (the data folder's, unless given) without changing it, reads
//! the shapes it hasn't kept yet from the project files (read only), and writes into the new or empty folder OUT:
//! - `stage-summary.txt`: counts and averages only, by status, the guesses against your statuses, and the limits that
//!   agree with you best. Nothing in it names a song: it's the file to send back.
//! - `stage-check.txt`: about 20 songs to look at together, each with your status, the guess and why.
//! - `stage-songs.csv`: every song's counts, for a closer look on your own computer.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use mo_app::live::{Live, LiveDetails};
use mo_formats::shape::Shape;
use mo_library::stage::{self, Limits};
use mo_library::{Library, STATUSES};
use rayon::prelude::*;

fn main() {
    if let Err(e) = run() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

/// One song as the tuner sees it: its status, its newest save's shape and whether it has a bounce newer than that.
struct Song {
    name: String,
    daw: String,
    status: &'static str,
    shape: Shape,
    bounced_since: bool,
    saved: i64,
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    let Some(out) = args.get(1) else {
        return Err("stagetune OUT [SONGS_DB]: OUT a new or empty folder; SONGS_DB the app's songs.db".into());
    };
    let out = PathBuf::from(out);
    if out.exists() && std::fs::read_dir(&out).map_err(|e| format!("{}: {e}", out.display()))?.next().is_some() {
        return Err(format!("{} isn't empty: pick a new folder, so nothing in it is written over", out.display()));
    }
    std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let db = args
        .get(2)
        .map(PathBuf::from)
        .unwrap_or_else(|| mo_app::paths::data_dir().join(mo_library::songs_db::FILE_NAME));
    let songs = measure(&db)?;
    let summary = summary(&songs);
    print!("{summary}");
    write(&out.join("stage-summary.txt"), &summary)?;
    let best = best_limits(&songs).first().map_or(Limits::USUAL, |(l, _)| *l);
    write(&out.join("stage-check.txt"), &to_check(&songs, &best))?;
    write(&out.join("stage-songs.csv"), &csv(&songs, &best))?;
    println!("\nWritten to {}", out.display());
    Ok(())
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Every song with a newest save that has a shape: the kept ones from songs.db, the rest read from their files.
fn measure(db: &Path) -> Result<Vec<Song>, String> {
    let library = Library::open_read_only(db).map_err(|e| format!("{}: {e}", db.display()))?;
    let mut live = Live::load(&library, mo_library::songs::DEFAULT_RUN_GAP_MONTHS, LiveDetails::default())
        .map_err(|e| format!("{}: {e}", db.display()))?;
    live.learn_shapes(&library).map_err(|e| format!("{}: {e}", db.display()))?;
    drop(library);
    let to_read = live.shapes_to_read();
    eprintln!("Reading {} newest saves' shapes from their files (the rest were kept already)…", to_read.len());
    let mut read = Vec::new();
    let total = to_read.len();
    mo_app::board::read_shapes(to_read, &Arc::new(AtomicBool::new(false)), |batch| {
        read.extend(batch);
        eprint!("\r{} of {total}", read.len());
    });
    eprintln!();
    // Held here only: Live has no songs.db to write to, so nothing is kept.
    live.set_shapes(read).map_err(|e| e.to_string())?;
    eprintln!("Looking for bounces beside the saves…");
    let rows = live.songs().rows;
    live.set_all_bounces(mo_app::bounces::find(&rows));
    let songs = live.songs();
    Ok(songs
        .rows
        .iter()
        .filter_map(|r| {
            let save = mo_app::board::newest(&r.song);
            // The guess's shape is gone from the row once made: worked out again from the kept one.
            let status = STATUSES.iter().find(|s| s.eq_ignore_ascii_case(&r.status)).copied()?;
            let shape = live.shape_of(save.id, save.mtime, save.size)?;
            Some(Song {
                name: r.song.name.clone(),
                daw: save.daw.clone(),
                status,
                bounced_since: r.bounces.iter().any(|b| b.mtime > save.mtime),
                shape,
                saved: save.mtime,
            })
        })
        .collect())
}

fn at(status: &str) -> usize {
    STATUSES.iter().position(|s| *s == status).unwrap_or(0)
}

/// The stage a status stands for when it's compared with a guess: Released is finished, and Archived says nothing.
fn compared(status: &str) -> Option<usize> {
    match at(status) {
        6 => None,
        5 => Some(4),
        n => Some(n),
    }
}

/// How well `limits` agree with your statuses, on the songs you've moved on from Idea (Idea is every new song's, so it
/// says little): how many get the stage you gave them, how many are guessed later (a move suggested wrongly), and how
/// many of your ideas would be offered a move.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Score {
    same: usize,
    later: usize,
    marked: usize,
    ideas_moved: usize,
}

fn score(songs: &[Song], limits: &Limits) -> Score {
    let mut s = Score::default();
    for song in songs {
        let guess = at(stage::stage_with(&song.shape, song.bounced_since, limits));
        match compared(song.status) {
            Some(0) => s.ideas_moved += usize::from(guess > 0),
            Some(mine) => {
                s.marked += 1;
                s.same += usize::from(guess == mine);
                s.later += usize::from(guess > mine);
            }
            None => {}
        }
    }
    s
}

/// Every limit tried, best first: most songs given the stage you gave them, then fewest moves suggested wrongly, then
/// fewest ideas offered a move.
fn best_limits(songs: &[Song]) -> Vec<(Limits, Score)> {
    let mut all = Vec::new();
    for idea_bars in [4.0, 8.0, 16.0, 24.0, 32.0] {
        for arranged_seconds in [60.0, 90.0, 120.0, 150.0, 180.0] {
            for mixed_tracks in [2, 3, 5, 8] {
                for mixed_eq in [0.2, 0.35, 0.5, 0.65] {
                    for mixed_compressed in [0.1, 0.2, 0.3, 0.45] {
                        for mixed_automation in [1, 2, 4, 8] {
                            for mixed_sends in [1, 2, 3] {
                                all.push(Limits {
                                    idea_bars,
                                    arranged_seconds,
                                    mixed_tracks,
                                    mixed_eq,
                                    mixed_compressed,
                                    mixed_automation,
                                    mixed_sends,
                                });
                            }
                        }
                    }
                }
            }
        }
    }
    let mut scored: Vec<(Limits, Score)> = all.into_par_iter().map(|l| (l, score(songs, &l))).collect();
    scored.sort_by(|(_, a), (_, b)| {
        b.same.cmp(&a.same).then(a.later.cmp(&b.later)).then(a.ideas_moved.cmp(&b.ideas_moved))
    });
    scored
}

fn quartiles(mut v: Vec<f64>) -> String {
    if v.is_empty() {
        return "-".into();
    }
    v.sort_by(f64::total_cmp);
    let q = |f: f64| v[((v.len() - 1) as f64 * f).round() as usize];
    format!("{:.1} / {:.1} / {:.1}", q(0.25), q(0.5), q(0.75))
}

fn share(n: u32, of: u32) -> f64 {
    if of == 0 { 0.0 } else { n as f64 / of as f64 }
}

fn summary(songs: &[Song]) -> String {
    let mut t = String::new();
    let _ = writeln!(t, "Stage tuner: {} songs with a measured newest save", songs.len());
    let _ = writeln!(t, "Quartiles (25% / median / 75%) by your status:\n");
    for status in STATUSES {
        let of: Vec<&Song> = songs.iter().filter(|s| s.status == status).collect();
        if of.is_empty() {
            continue;
        }
        let col = |f: &dyn Fn(&Song) -> f64| quartiles(of.iter().map(|s| f(s)).collect());
        let pct = |f: &dyn Fn(&Song) -> bool| 100 * of.iter().filter(|s| f(s)).count() / of.len();
        let _ = writeln!(t, "{status}: {} songs", of.len());
        let _ = writeln!(t, "  seconds      {}", col(&|s| s.shape.seconds));
        let _ = writeln!(t, "  bars         {}", col(&|s| s.shape.bars));
        let _ = writeln!(t, "  markers      {}", col(&|s| s.shape.markers as f64));
        let _ = writeln!(t, "  mixer tracks {}", col(&|s| s.shape.tracks as f64));
        let _ = writeln!(t, "  EQ share     {}", col(&|s| share(s.shape.eq, s.shape.tracks)));
        let _ = writeln!(t, "  comp share   {}", col(&|s| share(s.shape.compressed, s.shape.tracks)));
        let _ = writeln!(t, "  automation   {}", col(&|s| s.shape.automation as f64));
        let _ = writeln!(t, "  sends        {}", col(&|s| s.shape.sends as f64));
        let _ = writeln!(
            t,
            "  master limiter {}%, bounce newer than the save {}%, FL {}%",
            pct(&|s| s.shape.master_limited()),
            pct(&|s| s.bounced_since),
            pct(&|s| s.daw == "FL Studio")
        );
    }
    let _ = writeln!(t, "\nGuesses with today's limits, against your statuses (rows yours, columns guessed):");
    let mut table: BTreeMap<(usize, usize), usize> = BTreeMap::new();
    for s in songs {
        let g = at(stage::stage_with(&s.shape, s.bounced_since, &Limits::USUAL));
        *table.entry((at(s.status), g)).or_default() += 1;
    }
    let _ = write!(t, "{:>12}", "");
    for g in &STATUSES[..5] {
        let _ = write!(t, "{:>12}", g);
    }
    let _ = writeln!(t);
    for (mine, status) in STATUSES.iter().enumerate() {
        let _ = write!(t, "{status:>12}");
        for g in 0..5 {
            let _ = write!(t, "{:>12}", table.get(&(mine, g)).copied().unwrap_or(0));
        }
        let _ = writeln!(t);
    }
    let usual = score(songs, &Limits::USUAL);
    let _ = writeln!(t, "\nToday's limits: {}", said(&Limits::USUAL, &usual));
    let _ = writeln!(t, "\nBest limits tried (of songs past Idea: same stage as yours, guessed later than yours):");
    for (l, s) in best_limits(songs).iter().take(8) {
        let _ = writeln!(t, "  {}", said(l, s));
    }
    t
}

fn said(l: &Limits, s: &Score) -> String {
    format!(
        "same {}/{}, later {}, ideas offered a move {} | idea<{} bars, arranged>={}s, mixed: tracks>={} EQ>={} comp>={} \
         automation>={} or sends>={}",
        s.same,
        s.marked,
        s.later,
        s.ideas_moved,
        l.idea_bars,
        l.arranged_seconds,
        l.mixed_tracks,
        l.mixed_eq,
        l.mixed_compressed,
        l.mixed_automation,
        l.mixed_sends
    )
}

/// About 20 songs to look at together, newest first: ten where the guess with the best limits differs from your
/// status, and ten where it agrees, across the stages.
fn to_check(songs: &[Song], limits: &Limits) -> String {
    let mut order: Vec<&Song> = songs.iter().filter(|s| compared(s.status).is_some()).collect();
    order.sort_by_key(|s| std::cmp::Reverse(s.saved));
    let guessed = |s: &Song| stage::stage_with(&s.shape, s.bounced_since, limits);
    let differ: Vec<&Song> =
        order.iter().copied().filter(|s| compared(s.status) != Some(at(guessed(s)))).take(10).collect();
    let mut agree: Vec<&Song> = Vec::new();
    for stage_at in 0..5 {
        agree.extend(
            order
                .iter()
                .copied()
                .filter(|s| compared(s.status) == Some(at(guessed(s))) && at(guessed(s)) == stage_at)
                .take(2),
        );
    }
    let mut t = String::new();
    for (title, list) in [("Guess differs from your status", differ), ("Guess agrees", agree)] {
        let _ = writeln!(t, "{title}:\n");
        for s in list {
            let g = stage::guess_with(&s.shape, s.bounced_since, limits);
            let _ = writeln!(t, "- {} ({}): yours {}, guessed {}", s.name, s.daw, s.status, g.stage);
            for why in g.why {
                let _ = writeln!(t, "    {why}");
            }
        }
        let _ = writeln!(t);
    }
    t
}

fn csv(songs: &[Song], limits: &Limits) -> String {
    let mut t = String::from(
        "song,daw,status,guess,seconds,bars,markers,tracks,eq,compressed,sends,automation,master limiter,bounced since\n",
    );
    for s in songs {
        let sh = &s.shape;
        let _ = writeln!(
            t,
            "\"{}\",{},{},{},{:.0},{:.1},{},{},{},{},{},{},\"{}\",{}",
            s.name.replace('"', "\"\""),
            s.daw,
            s.status,
            stage::stage_with(sh, s.bounced_since, limits),
            sh.seconds,
            sh.bars,
            sh.markers,
            sh.tracks,
            sh.eq,
            sh.compressed,
            sh.sends,
            sh.automation,
            sh.master_limiter().unwrap_or_default().replace('"', "\"\""),
            s.bounced_since
        );
    }
    t
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn song(status: &'static str) -> Song {
        Song {
            name: status.into(),
            daw: "FL Studio".into(),
            status,
            shape: Shape::default(),
            bounced_since: false,
            saved: 0,
        }
    }

    #[test]
    fn released_counts_as_finished_and_archived_is_left_out() {
        assert_eq!(compared("Released"), Some(4));
        assert_eq!(compared("Finished"), Some(4));
        assert_eq!(compared("Archived"), None);
        assert_eq!(compared("Idea"), Some(0));
    }

    #[test]
    fn an_empty_save_is_an_idea_so_only_marked_songs_count_against_it() {
        let songs = [song("Idea"), song("Mixing"), song("Archived")];
        let s = score(&songs, &Limits::USUAL);
        assert_eq!(s, Score { same: 0, later: 0, marked: 1, ideas_moved: 0 });
    }
}
