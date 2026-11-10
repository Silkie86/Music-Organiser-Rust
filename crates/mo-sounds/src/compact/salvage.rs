//! What can still be read of a damaged sound library, kept (review findings S-1 and S-3, 10 Nov). The app's own library
//! found damaged is put back from its backup copy, or (with none to use) made again from `sounds.db`
//! ([`super::mend_reading`]). Either way the damaged one is set aside ([`super::backup::damaged_of`]), never removed,
//! and everything in it that can still be read goes into the new one before that has its name: the sounds the app's
//! scan added, your favourites, stars, own types and tags, plays, drags and sends, the galaxy's maps. Where the two
//! differ, the damaged library's word wins (it's the newer: the backup copy and `sounds.db` are what it was made from,
//! or a copy of it from before); a row it no longer has (a sound forgotten, a tag taken away) goes, and a sound
//! forgotten takes its rows in the other tables with it. Where part of it can't be read, the new library keeps what the
//! backup copy or `sounds.db` had there; a row kept from the damaged library that points at a sound the new one hasn't
//! got (one the scan added, in a part that can't be read) goes. A waveform or fingerprint kept from the backup copy or
//! `sounds.db` may not match the sound's row until the sound is listened to again: the scan does so for a file whose
//! size or time the row has wrong.
//!
//! Each table is gone through by row number, a few hundred of the damaged library's rows at a time, as they come;
//! where they can't be read, fewer at a time, down to one. Past a row that can't be read, it looks further on, a
//! stretch of row numbers and then twice as far each time, for where rows can be read again, then back to the first
//! of them: only the rows in between are left out.
//!
//! The damaged library is read on a connection of its own, opened only to read: nothing is ever written into it, and
//! SQLite gives up the new library's whole transaction when a read fails on the disk, which a read on the same
//! connection would do.

use std::collections::BTreeMap;
use std::path::Path;

use rusqlite::types::{ToSqlOutput, ValueRef};
use rusqlite::{Connection, OptionalExtension, ToSql, params, params_from_iter};

use super::backup::{BACKED_UP_AT, BACKED_UP_FROM, open_read_only};
use super::grow::{LAST_ID, NEW_SINCE};
use super::{Columns, FORMAT, Failed, INFO_TABLE, Room, index_sqls, quoted, table_sql, without_rowid};
use crate::db::is_damage;

/// What was kept of a damaged library ([`salvage`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Salvaged {
    /// Changes made to the new library (the backup copy, or what `sounds.db` gave): rows of the damaged library put in
    /// where they differed, and rows it no longer had taken out.
    pub kept: usize,
    /// Places where it couldn't be read: a stretch of rows, a whole table, or all of it.
    pub unread: usize,
}

/// How many rows are read from a table at a time, and how far on (in row numbers) it first looks past rows that can't
/// be read.
const STRETCH: i64 = 256;

/// Puts into the new library open at `ours` (made from `sounds.db`, or put back from its backup copy, and not yet
/// given its name) what can be read of the damaged one at `damaged`. The disk is watched for room in `dir`. Damage met
/// in it leaves that part out; anything else (the disk full, the file gone) stops it, and the new library with it.
/// `ours` checks no foreign keys from then on (the tables copied whole say a row goes with its sound, and putting one
/// sound's row in would take its tags and plays with it).
pub(super) fn salvage(ours: &Connection, damaged: &Path, dir: &Path) -> Result<Salvaged, Failed> {
    // A long read, beside the app's other reads of the library (the waveforms on show go in between).
    let _turn = crate::turns::long_turn();
    // Into a library of this version's format only: a backup copy of the format before is put back as it is, and
    // what came since can't go into it.
    if super::format_of(ours)? != Some(FORMAT) {
        return Ok(Salvaged { kept: 0, unread: 1 });
    }
    ours.pragma_update(None, "foreign_keys", false)?;
    let theirs = open_read_only(damaged)?;
    // One of another layout keeps things where this one doesn't (CLAP's numbers as CLAP gave them, say), and the app
    // only ever writes into one of this layout: such a one holds nothing of the app's own to keep.
    let tables = match this_format(&theirs).and_then(|this| if this { tables(&theirs) } else { Ok(Vec::new()) }) {
        Ok(tables) => tables,
        Err(e) if is_damage(&e) => return Ok(Salvaged { kept: 0, unread: 1 }),
        Err(e) => return Err(e.into()),
    };
    if tables.is_empty() {
        return Ok(Salvaged::default());
    }
    let highest_before = highest_sound(ours)?;
    let mut pass = Pass {
        ours,
        theirs: &theirs,
        out: Salvaged::default(),
        room: Room::new(dir),
        put: BTreeMap::new(),
        forgotten: Vec::new(),
    };
    // One transaction, so the rows go onto the disk together (should this stop, the new library is thrown away whole).
    ours.execute_batch("BEGIN")?;
    // The sounds first: those forgotten take their rows in the other tables with them.
    let (sounds, others): (Vec<_>, Vec<_>) = tables.into_iter().partition(|(table, _)| table == "sounds");
    for (table, made) in sounds.into_iter().chain(others) {
        if table == INFO_TABLE {
            pass.notes_into()?;
        } else {
            pass.table_into(&table, &made)?;
        }
    }
    pass.orphans_out()?;
    pass.counts_into()?;
    numbers_noted(ours, highest_before)?;
    ours.execute_batch("COMMIT")?;
    Ok(pass.out)
}

/// Whether the damaged library is laid out in this version's format: it has the waveforms' own table, and not the row
/// hashes of the format before, and notes this format (or its note can't be read).
fn this_format(theirs: &Connection) -> rusqlite::Result<bool> {
    let has = |table: &str| {
        theirs.query_row("SELECT COUNT(*) > 0 FROM sqlite_master WHERE type = 'table' AND name = ?1", [table], |r| {
            r.get::<_, bool>(0)
        })
    };
    if !has("sound_waves")? || has("sound_hashes")? || !has(INFO_TABLE)? {
        return Ok(false);
    }
    match super::format_of(theirs) {
        Ok(Some(format)) => Ok(format == FORMAT),
        Ok(None) => Ok(true),
        Err(e) if is_damage(&e) => Ok(true),
        Err(e) => Err(e),
    }
}

/// The damaged library's tables, with what made each (one whose name or making isn't text is left out).
fn tables(theirs: &Connection) -> rusqlite::Result<Vec<(String, String)>> {
    let mut st = theirs.prepare(
        "SELECT name, sql FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' AND sql IS NOT NULL",
    )?;
    let text = |v: ValueRef<'_>| v.as_str().ok().map(str::to_owned);
    let mut rows = st.query([])?;
    let mut tables = Vec::new();
    while let Some(r) = rows.next()? {
        if let (Some(name), Some(made)) = (text(r.get_ref(0)?), text(r.get_ref(1)?)) {
            tables.push((name, made));
        }
    }
    Ok(tables)
}

/// The highest number a sound has in the library at `conn`.
fn highest_sound(conn: &Connection) -> rusqlite::Result<Option<i64>> {
    conn.query_row("SELECT MAX(id) FROM main.sounds", [], |r| r.get(0))
}

/// The numbers the app gave its sounds, noted again as the new library has them now: the highest given
/// ([`LAST_ID`]) at least its highest sound, and (when the damaged library's note of it couldn't be read) the first
/// number the app gave a sound of its own scan ([`NEW_SINCE`]), for one kept above `before`, the highest the new
/// library had.
fn numbers_noted(ours: &Connection, before: Option<i64>) -> rusqlite::Result<()> {
    let Some(highest) = highest_sound(ours)? else { return Ok(()) };
    let noted = super::info_value(ours, LAST_ID)?;
    if noted.is_none_or(|noted| noted < highest) {
        ours.execute("INSERT OR REPLACE INTO main.copy_info (key, value) VALUES (?1, ?2)", params![LAST_ID, highest])?;
    }
    if super::info_value(ours, NEW_SINCE)?.is_none() {
        let first: Option<i64> =
            ours.query_row("SELECT MIN(id) FROM main.sounds WHERE id > ?1", [before.unwrap_or(0)], |r| r.get(0))?;
        if let Some(first) = first {
            ours.execute("INSERT INTO main.copy_info (key, value) VALUES (?1, ?2)", params![NEW_SINCE, first])?;
        }
    }
    Ok(())
}

/// Rows by row number, each with its columns' values.
type Rows = Vec<(i64, Vec<Cell>)>;

/// One salvage: the new library (`ours`), the damaged one (`theirs`), what's been kept so far, the disk watched for
/// room, the rows put in, by table (for those that point at a sound the new library hasn't got to go again), and the
/// sounds the damaged library forgot (for their rows in the other tables to go with them).
struct Pass<'a> {
    ours: &'a Connection,
    theirs: &'a Connection,
    out: Salvaged,
    room: Room<'a>,
    put: BTreeMap<String, Vec<i64>>,
    forgotten: Vec<i64>,
}

impl Pass<'_> {
    /// What can be read of one damaged `table` (made by `made`), into the new library's: made there first, as the
    /// damaged library made it, when the new one has none (one the app's own writes made, say).
    fn table_into(&mut self, table: &str, made: &str) -> Result<(), Failed> {
        let mine = table_sql(self.ours, "main", table)?;
        let new_table = mine.is_none();
        if new_table {
            self.ours.execute_batch(made)?;
        }
        let mine = mine.as_deref().unwrap_or(made);
        // The columns both have, in the damaged library's order.
        let have = Columns::of(self.ours, "main", table, mine)?;
        let columns = match Columns::of(self.theirs, "main", table, made) {
            Ok(columns) => columns.names.into_iter().filter(|c| have.names.contains(c)).collect::<Vec<_>>(),
            Err(e) if is_damage(&e) => {
                self.out.unread += 1;
                return Ok(());
            }
            Err(e) => return Err(e.into()),
        };
        if !columns.is_empty() {
            let t = Table::new(table, columns);
            if without_rowid(made) || without_rowid(mine) { self.whole_into(&t)? } else { self.walk(&t)? }
        }
        if new_table {
            let indexes = match index_sqls(self.theirs, "main", table) {
                Ok(indexes) => indexes,
                Err(e) if is_damage(&e) => Vec::new(),
                Err(e) => return Err(e.into()),
            };
            for sql in indexes {
                self.ours.execute_batch(&sql)?;
            }
        }
        Ok(())
    }

    /// Goes through a table with row numbers, from its lowest, the damaged library's rows as they come
    /// ([`Pass::next_rows`]), past the rows that can't be read ([`Pass::readable_from`]), up to the highest number
    /// either library has. None of it when the damaged library's lowest and highest can't be found (its first page is
    /// damaged, say).
    fn walk(&mut self, t: &Table) -> Result<(), Failed> {
        let end = |db: &Connection, which: &str| -> rusqlite::Result<Option<i64>> {
            db.query_row(&format!("SELECT {which}(rowid) FROM main.{}", t.quoted), [], |r| r.get(0))
        };
        let their_end = |which: &str| match end(self.theirs, which) {
            Ok(n) => Ok(Some(n)),
            Err(e) if is_damage(&e) => Ok(None),
            Err(e) => Err(e),
        };
        // Each `None` when it can't be read; `Some(None)` when the table has no rows.
        let (their_low, their_high) = (their_end("MIN")?, their_end("MAX")?);
        if their_low.is_none() && their_high.is_none() {
            self.out.unread += 1;
            return Ok(());
        }
        let (my_low, my_high) = (end(self.ours, "MIN")?, end(self.ours, "MAX")?);
        // The app's row numbers start at 1: from there when the damaged library's lowest can't be read.
        let mut from = [my_low, their_low.flatten(), Some(1)].into_iter().flatten().min().unwrap_or(1);
        let highest = [my_high, their_high.flatten()].into_iter().flatten().max();
        loop {
            match self.next_rows(t, from, STRETCH)? {
                Some((rows, ended)) => {
                    let last = rows.last().map(|&(number, _)| number);
                    // Ended before the highest row it says it has: what's after can't be read after all.
                    let short =
                        their_high.flatten().filter(|&high| ended && last.map_or(from <= high, |last| last < high));
                    let to = match (ended, short) {
                        (true, None) => i64::MAX,
                        _ => last.unwrap_or(i64::MIN),
                    };
                    if to >= from {
                        self.merge(t, (from, to), &rows)?;
                    }
                    if let Some(high) = short {
                        // The new library's rows up to there stay as they are.
                        self.out.unread += 1;
                        let Some(after) = high.checked_add(1) else { break };
                        from = after;
                    } else {
                        match to.checked_add(1) {
                            Some(next) if to != i64::MAX => from = next,
                            _ => break,
                        }
                    }
                }
                // The row after `from` can't be read: on to where rows can be again, the new library's rows in
                // between staying as they are.
                None => {
                    let Some(highest) = highest.filter(|&highest| from <= highest) else {
                        // Past every row either library has, as far as is known.
                        if their_high.is_none() {
                            self.out.unread += 1;
                        }
                        break;
                    };
                    self.out.unread += 1;
                    match self.readable_from(t, from, highest)? {
                        Some(again) => from = again,
                        None => break,
                    }
                }
            }
            crate::turns::give_way();
        }
        Ok(())
    }

    /// The damaged library's next `n` rows of `t` from row number `from`, or fewer where they can't be read, and
    /// whether the table ends with them. `None` when the very next can't be read.
    fn next_rows(&self, t: &Table, from: i64, n: i64) -> Result<Option<(Rows, bool)>, Failed> {
        let mut n = n;
        loop {
            match read(self.theirs, &t.from, (from, n), t.columns.len()) {
                Ok(rows) => {
                    let ended = i64::try_from(rows.len()).unwrap_or(i64::MAX) < n;
                    return Ok(Some((rows, ended)));
                }
                Err(e) if is_damage(&e) => {
                    if n == 1 {
                        return Ok(None);
                    }
                    n /= 2;
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// The first row number past `from` (whose next row can't be read) from which the damaged library's next row can
    /// be: looked for a stretch on, then twice as far each time, up to just past `highest`, and then back between the
    /// last that couldn't and the first that could. `None` when none can, up to there.
    fn readable_from(&self, t: &Table, from: i64, highest: i64) -> Result<Option<i64>, Failed> {
        let readable = |at: i64| self.next_rows(t, at, 1).map(|rows| rows.is_some());
        let limit = highest.saturating_add(1);
        let (mut bad, mut step) = (from, STRETCH);
        let mut good = loop {
            let at = bad.saturating_add(step).min(limit);
            if readable(at)? {
                break at;
            }
            if at >= limit {
                return Ok(None);
            }
            bad = at;
            step = step.saturating_mul(2);
        };
        while good - bad > 1 {
            let middle = bad + (good - bad) / 2;
            if readable(middle)? {
                good = middle;
            } else {
                bad = middle;
            }
        }
        Ok(Some(good))
    }

    /// The damaged library's rows of `t` numbered `from` to `to` (`theirs`, every one it has there, by number) into
    /// the new library: each that differs put in, and the new library's between them, which the damaged one no longer
    /// has, taken out (for the sounds, noted as forgotten).
    fn merge(&mut self, t: &Table, (from, to): (i64, i64), theirs: &Rows) -> Result<(), Failed> {
        let mut gaps = Vec::new();
        let mut low = Some(from);
        for &(number, _) in theirs {
            if let Some(low) = low.filter(|&low| low < number) {
                gaps.push((low, number - 1));
            }
            low = number.checked_add(1);
        }
        if let Some(low) = low.filter(|&low| low <= to) {
            gaps.push((low, to));
        }
        let sounds = t.name == "sounds";
        let mut which = self.ours.prepare_cached(&t.numbers_between)?;
        let mut gone = self.ours.prepare_cached(&t.delete_between)?;
        for gap in gaps {
            if sounds {
                let numbers = which.query_map(gap, |r| r.get(0))?.collect::<rusqlite::Result<Vec<i64>>>()?;
                self.forgotten.extend(numbers);
            }
            self.out.kept += gone.execute(gap)?;
        }
        let mut mine = self.ours.prepare_cached(&t.at)?;
        let mut put = self.ours.prepare_cached(&t.put)?;
        for (number, values) in theirs {
            let have = rows_of(&mut mine, [number], t.columns.len())?;
            if have.first().map(|(_, v)| v) != Some(values) {
                put.execute(params_from_iter(values))?;
                self.out.kept += 1;
                self.put.entry(t.name.clone()).or_default().push(*number);
                self.room.wrote()?;
            }
        }
        Ok(())
    }

    /// A table without row numbers, which there are none to go through it by: the damaged library's rows in place of
    /// the new one's, all of them, or (any of it damaged) none.
    fn whole_into(&mut self, t: &Table) -> Result<(), Failed> {
        let sql = format!("SELECT 0, {} FROM main.{}", t.list, t.quoted);
        let their_rows = match read(self.theirs, &sql, [], t.columns.len()) {
            Ok(rows) => rows,
            Err(e) if is_damage(&e) => {
                self.out.unread += 1;
                return Ok(());
            }
            Err(e) => return Err(e.into()),
        };
        let my_rows = read(self.ours, &sql, [], t.columns.len())?;
        // Both in the order of their keys, which is how SQLite keeps a table without row numbers.
        let same = their_rows.iter().zip(&my_rows).filter(|(a, b)| a == b).count();
        let differ = their_rows.len().max(my_rows.len()) - same;
        if differ == 0 {
            return Ok(());
        }
        self.ours.execute_batch(&format!("DELETE FROM main.{}", t.quoted))?;
        let mut put = self.ours.prepare(&t.put)?;
        for (_, values) in &their_rows {
            put.execute(params_from_iter(values))?;
            self.room.wrote()?;
        }
        self.out.kept += differ;
        Ok(())
    }

    /// The damaged library's notes (`copy_info`), by what each is of: each it has put in where it differs (but what a
    /// backup copy notes of itself). None taken out: the new library's own notes it doesn't have stay.
    fn notes_into(&mut self) -> Result<(), Failed> {
        let notes = (|| -> rusqlite::Result<Vec<(Cell, Cell)>> {
            let mut st = self.theirs.prepare("SELECT key, value FROM copy_info")?;
            let mut rows = st.query([])?;
            let mut notes = Vec::new();
            while let Some(r) = rows.next()? {
                notes.push((Cell::of(r.get_ref(0)?), Cell::of(r.get_ref(1)?)));
            }
            Ok(notes)
        })();
        let notes = match notes {
            Ok(notes) => notes,
            Err(e) if is_damage(&e) => {
                self.out.unread += 1;
                return Ok(());
            }
            Err(e) => return Err(e.into()),
        };
        let backups = [BACKED_UP_FROM, BACKED_UP_AT].map(|key| Cell::Text(key.as_bytes().to_vec()));
        let mut mine = self.ours.prepare("SELECT value FROM main.copy_info WHERE key = ?1")?;
        let mut put = self.ours.prepare("INSERT OR REPLACE INTO main.copy_info (key, value) VALUES (?1, ?2)")?;
        for (key, value) in notes {
            if backups.contains(&key) {
                continue;
            }
            let have = mine.query_row([&key], |r| Ok(Cell::of(r.get_ref(0)?))).optional()?;
            if have.as_ref() != Some(&value) {
                put.execute([&key, &value])?;
                self.out.kept += 1;
            }
        }
        Ok(())
    }

    /// The rows that point at a sound the new library hasn't got go: those the damaged library forgot take theirs with
    /// them (they may come from a part of it that couldn't be read), and a row put in for a sound in a part of the
    /// sounds that couldn't be read would only be counted. Rows of no sound the new library had before stay as they
    /// were.
    fn orphans_out(&mut self) -> Result<(), Failed> {
        let tables: Vec<(String, String)> = {
            let mut st = self.ours.prepare(
                "SELECT name, sql FROM main.sqlite_master
                 WHERE type = 'table' AND name NOT LIKE 'sqlite_%' AND name <> 'sounds' AND sql IS NOT NULL",
            )?;
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?
        };
        for (table, made) in tables {
            // Its columns that name a sound: those the table says refer to one, and the app's own `sound_id`.
            let mut st = self.ours.prepare(
                "SELECT \"from\" FROM pragma_foreign_key_list(?1) WHERE \"table\" = 'sounds'
                 UNION SELECT name FROM pragma_table_info(?1) WHERE name = 'sound_id'",
            )?;
            let pointers: Vec<String> = st.query_map([&table], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
            if pointers.is_empty() {
                continue;
            }
            let q = quoted(&table);
            for c in pointers.iter().map(|c| quoted(c)) {
                let mut gone = self.ours.prepare(&format!("DELETE FROM main.{q} WHERE {c} = ?1"))?;
                for number in &self.forgotten {
                    self.out.kept += gone.execute([number])?;
                }
            }
            let Some(numbers) = self.put.get(&table).filter(|_| !without_rowid(&made)) else { continue };
            let none = pointers
                .iter()
                .map(|c| {
                    let c = quoted(c);
                    format!("({c} IS NOT NULL AND NOT EXISTS (SELECT 1 FROM main.sounds WHERE id = {c}))")
                })
                .collect::<Vec<_>>()
                .join(" OR ");
            let mut gone = self.ours.prepare(&format!("DELETE FROM main.{q} WHERE rowid = ?1 AND ({none})"))?;
            for number in numbers {
                gone.execute([number])?;
            }
        }
        Ok(())
    }

    /// SQLite's own count for each table whose rows count up (AUTOINCREMENT), moved on to the damaged library's where
    /// that's further: a number it gave is never given again.
    fn counts_into(&mut self) -> Result<(), Failed> {
        if table_sql(self.ours, "main", "sqlite_sequence")?.is_none() {
            return Ok(());
        }
        let counts = (|| -> rusqlite::Result<Vec<(String, i64)>> {
            if table_sql(self.theirs, "main", "sqlite_sequence")?.is_none() {
                return Ok(Vec::new());
            }
            let mut st = self.theirs.prepare("SELECT name, seq FROM sqlite_sequence WHERE typeof(seq) = 'integer'")?;
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect()
        })();
        let counts = match counts {
            Ok(counts) => counts,
            Err(e) if is_damage(&e) => {
                self.out.unread += 1;
                return Ok(());
            }
            Err(e) => return Err(e.into()),
        };
        for (table, seq) in counts {
            if table_sql(self.ours, "main", &table)?.is_none() {
                continue;
            }
            let moved = self
                .ours
                .execute("UPDATE main.sqlite_sequence SET seq = MAX(seq, ?2) WHERE name = ?1", (&table, seq))?;
            if moved == 0 {
                self.ours.execute("INSERT INTO main.sqlite_sequence (name, seq) VALUES (?1, ?2)", (&table, seq))?;
            }
        }
        Ok(())
    }
}

/// A table gone through: its name, quoted, the columns both libraries have, and what reads and writes its rows.
struct Table {
    name: String,
    quoted: String,
    columns: Vec<String>,
    list: String,
    /// Up to `?2` rows from row number `?1` on, by number.
    from: String,
    /// The row numbered `?1`.
    at: String,
    /// The numbers of the rows numbered `?1` to `?2`.
    numbers_between: String,
    /// Takes out the rows numbered `?1` to `?2`.
    delete_between: String,
    /// Puts a row in, in place of one with its number (or its keys).
    put: String,
}

impl Table {
    fn new(name: &str, columns: Vec<String>) -> Table {
        let q = quoted(name);
        let list = columns.join(", ");
        let places = vec!["?"; columns.len()].join(", ");
        Table {
            name: name.to_owned(),
            from: format!("SELECT rowid, {list} FROM main.{q} WHERE rowid >= ?1 ORDER BY rowid LIMIT ?2"),
            at: format!("SELECT rowid, {list} FROM main.{q} WHERE rowid = ?1"),
            numbers_between: format!("SELECT rowid FROM main.{q} WHERE rowid BETWEEN ?1 AND ?2"),
            delete_between: format!("DELETE FROM main.{q} WHERE rowid BETWEEN ?1 AND ?2"),
            put: format!("INSERT OR REPLACE INTO main.{q} ({list}) VALUES ({places})"),
            quoted: q,
            columns,
            list,
        }
    }
}

/// The rows `sql` gives from `db` with `params`: each a row number, then `columns` values.
fn read(db: &Connection, sql: &str, params: impl rusqlite::Params, columns: usize) -> rusqlite::Result<Rows> {
    let mut st = db.prepare_cached(sql)?;
    rows_of(&mut st, params, columns)
}

fn rows_of(st: &mut rusqlite::Statement<'_>, params: impl rusqlite::Params, columns: usize) -> rusqlite::Result<Rows> {
    let mut rows = st.query(params)?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let values = (1..=columns).map(|i| r.get_ref(i).map(Cell::of)).collect::<rusqlite::Result<Vec<_>>>()?;
        out.push((r.get(0)?, values));
    }
    Ok(out)
}

/// A value as SQLite keeps it, its text as bytes: a damaged library's text may not be UTF-8, and is kept as it is.
#[derive(Debug)]
enum Cell {
    Null,
    Integer(i64),
    Real(f64),
    Text(Vec<u8>),
    Blob(Vec<u8>),
}

impl Cell {
    fn of(value: ValueRef<'_>) -> Cell {
        match value {
            ValueRef::Null => Cell::Null,
            ValueRef::Integer(n) => Cell::Integer(n),
            ValueRef::Real(x) => Cell::Real(x),
            ValueRef::Text(t) => Cell::Text(t.to_vec()),
            ValueRef::Blob(b) => Cell::Blob(b.to_vec()),
        }
    }
}

/// The same to the last bit: a -0.0 isn't a 0.0.
impl PartialEq for Cell {
    fn eq(&self, other: &Cell) -> bool {
        match (self, other) {
            (Cell::Null, Cell::Null) => true,
            (Cell::Integer(a), Cell::Integer(b)) => a == b,
            (Cell::Real(a), Cell::Real(b)) => a.to_bits() == b.to_bits(),
            (Cell::Text(a), Cell::Text(b)) | (Cell::Blob(a), Cell::Blob(b)) => a == b,
            _ => false,
        }
    }
}

impl ToSql for Cell {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::Borrowed(match self {
            Cell::Null => ValueRef::Null,
            Cell::Integer(n) => ValueRef::Integer(*n),
            Cell::Real(x) => ValueRef::Real(*x),
            Cell::Text(t) => ValueRef::Text(t),
            Cell::Blob(b) => ValueRef::Blob(b),
        }))
    }
}
