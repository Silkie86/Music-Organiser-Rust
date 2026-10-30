//! The twin test (spec section 11.2): reads the same project files with the C# app's readers (through the small C#
//! helper in `cs/`) and with the Rust ones, and lists every place they differ. Each difference is then judged in
//! `crates/mo-formats/DIFFERENCES.md`: the Rust reader is better, or it's a bug to fix.
//!
//! ```text
//! twin run --cs <helper> <folder or file>... [--report <file>] [--expect <file>] [--write-expect <file>]
//! twin dump <file>
//! ```
//!
//! `run` prints a summary that holds only counts and field names (no names, paths or notes from the songs), so it's safe
//! to paste into a conversation; every difference, file by file, goes in the report. `--expect` checks the differences
//! against a list of the ones already judged and fails on any new one (CI uses it on the test files); `--write-expect`
//! writes that list. `dump` prints what the Rust readers find in one file.

mod compare;
mod rust_side;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use rayon::prelude::*;
use serde_json::Value;
use sha2::{Digest, Sha256};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("dump") if args.len() == 2 => {
            println!("{:#}", rust_side::dump(Path::new(&args[1])));
            Ok(true)
        }
        Some("run") => run(&args[1..]),
        _ => Err(USAGE.to_owned()),
    };
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(2)
        }
    }
}

const USAGE: &str =
    "twin run --cs <helper> <folder or file>... [--report <file>] [--expect <file>] [--write-expect <file>]
twin dump <file>";

/// How many files are read (by both apps) before their readings are compared and let go.
const CHUNK: usize = 256;
/// A file the C# helper said nothing about, which counts as a difference.
const NO_CS: &str = "(no C# reading)";

/// The top of an `--expect` list.
const EXPECT_HEADER: &str =
    "# The twin test's differences on the test files, each judged in crates/mo-formats/DIFFERENCES.md.
# A file name, the kind of difference, and a fingerprint of what both apps found there, apart by tabs.
# Written by `twin run ... --write-expect`.
";

#[derive(Default)]
struct Options {
    cs: Option<PathBuf>,
    report: Option<PathBuf>,
    expect: Option<PathBuf>,
    write_expect: Option<PathBuf>,
    inputs: Vec<PathBuf>,
}

fn options(args: &[String]) -> Result<Options, String> {
    let mut o = Options::default();
    let mut args = args.iter();
    while let Some(a) = args.next() {
        let mut value = || args.next().map(PathBuf::from).ok_or_else(|| format!("{a} needs a value\n{USAGE}"));
        match a.as_str() {
            "--cs" => o.cs = Some(value()?),
            "--report" => o.report = Some(value()?),
            "--expect" => o.expect = Some(value()?),
            "--write-expect" => o.write_expect = Some(value()?),
            _ => o.inputs.push(PathBuf::from(a)),
        }
    }
    if o.cs.is_none() || o.inputs.is_empty() {
        return Err(USAGE.to_owned());
    }
    Ok(o)
}

/// Runs the twin test. `Ok(false)` when `--expect` found differences not on its list (or some gone from it).
fn run(args: &[String]) -> Result<bool, String> {
    let o = options(args)?;
    let mut files = Vec::new();
    for input in &o.inputs {
        collect(input, &mut files);
    }
    files.sort();
    files.dedup();
    if files.is_empty() {
        return Err("No FL Studio or Bitwig projects (.flp, .bwproject) there.".into());
    }

    let helper = o.cs.clone().ok_or("Say where the C# helper is (--cs).")?;
    let mut report_file = match &o.report {
        Some(path) => Some(std::io::BufWriter::new(
            std::fs::File::create(path).map_err(|e| format!("Couldn't write the report: {e}"))?,
        )),
        None => None,
    };
    // Kept only when there's no report file to write to (CI prints it).
    let mut report = String::new();
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    let mut found: BTreeSet<String> = BTreeSet::new();
    let mut same = 0;
    // A few hundred files at a time, so a big library's readings never all sit in memory at once.
    for chunk in files.chunks(CHUNK) {
        // The C# helper reads in its own process while the Rust readers read here, on every core.
        let cs_files = chunk.to_vec();
        let helper = helper.clone();
        let cs = std::thread::spawn(move || run_helper(&helper, &cs_files));
        let rust: Vec<Value> = chunk.par_iter().map(|f| rust_side::dump(f)).collect();
        let cs = cs.join().map_err(|_| "The C# helper's reader thread stopped.".to_owned())??;

        let mut part = String::new();
        for (file, rust) in chunk.iter().zip(&rust) {
            let key = file.to_string_lossy().into_owned();
            let name = file.file_name().map_or(key.clone(), |n| n.to_string_lossy().into_owned());
            let Some(cs) = cs.get(&key) else {
                part += &format!("{key}\n  the C# helper said nothing about it\n\n");
                found.insert(format!("{name}\t{NO_CS}"));
                *kinds.entry(NO_CS.into()).or_default() += 1;
                continue;
            };
            let diffs = compare::differences(cs, rust);
            if diffs.is_empty() {
                same += 1;
                continue;
            }
            part += &format!("{key}\n");
            // Each kind of difference with what both sides had, so the list in --expect notices a value changing.
            let mut file_kinds: BTreeMap<String, Sha256> = BTreeMap::new();
            for d in &diffs {
                part += &format!("  {}\n    C#:   {}\n    Rust: {}\n", d.path, d.cs, d.rust);
                file_kinds.entry(d.kind()).or_default().update(format!("{}\t{}\t{}\n", d.path, d.cs, d.rust));
            }
            part += "\n";
            for (k, hash) in file_kinds {
                let hash: String = hash.finalize().iter().take(4).map(|b| format!("{b:02x}")).collect();
                found.insert(format!("{name}\t{k}\t{hash}"));
                *kinds.entry(k).or_default() += 1;
            }
        }
        match &mut report_file {
            Some(f) => f.write_all(part.as_bytes()).map_err(|e| format!("Couldn't write the report: {e}"))?,
            None => report += &part,
        }
    }
    if let Some(mut f) = report_file {
        f.flush().map_err(|e| format!("Couldn't write the report: {e}"))?;
    }

    let bitwig = files.iter().filter(|f| f.extension().is_some_and(|e| e.eq_ignore_ascii_case("bwproject"))).count();
    println!("Twin test: {} FL Studio and {bitwig} Bitwig project(s).", files.len() - bitwig);
    println!("  {same} read the same in both apps.");
    println!("  {} differ. Kinds of difference, by how many projects have each:", files.len() - same);
    let mut by_count: Vec<(&String, &usize)> = kinds.iter().collect();
    by_count.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    for (kind, n) in by_count {
        println!("  {n:>7}  {kind}");
    }
    if let Some(path) = &o.report {
        println!("Every difference, file by file: {}", path.display());
    }
    if let Some(path) = &o.write_expect {
        let list: String =
            std::iter::once(EXPECT_HEADER.to_owned()).chain(found.iter().map(|l| format!("{l}\n"))).collect();
        std::fs::write(path, list).map_err(|e| format!("Couldn't write the list: {e}"))?;
    }
    let Some(path) = &o.expect else { return Ok(true) };
    let text = std::fs::read_to_string(path).map_err(|e| format!("Couldn't read {}: {e}", path.display()))?;
    let expected: BTreeSet<String> =
        text.lines().filter(|l| !l.trim().is_empty() && !l.starts_with('#')).map(str::to_owned).collect();
    let new: Vec<&String> = found.difference(&expected).collect();
    let gone: Vec<&String> = expected.difference(&found).collect();
    for l in &new {
        println!("New difference (judge it in DIFFERENCES.md, then add it to {}): {l}", path.display());
    }
    for l in &gone {
        println!("No longer different (take it off {}): {l}", path.display());
    }
    if !new.is_empty() && o.report.is_none() {
        print!("{report}");
    }
    Ok(new.is_empty() && gone.is_empty())
}

/// The FL Studio and Bitwig projects in a folder (and the folders in it), or the file itself.
fn collect(path: &Path, out: &mut Vec<PathBuf>) {
    if path.is_dir() {
        let Ok(entries) = std::fs::read_dir(path) else { return };
        for entry in entries.flatten() {
            collect(&entry.path(), out);
        }
    } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("flp") || e.eq_ignore_ascii_case("bwproject")) {
        out.push(path.to_path_buf());
    }
}

/// Runs the C# helper on the files (one path a line on its input) and takes its readings (one JSON a line), by file.
/// A helper ending in `.dll` is run with `dotnet`.
fn run_helper(helper: &Path, files: &[PathBuf]) -> Result<HashMap<String, Value>, String> {
    let mut command = if helper.extension().is_some_and(|e| e.eq_ignore_ascii_case("dll")) {
        let mut c = Command::new("dotnet");
        c.arg(helper);
        c
    } else {
        Command::new(helper)
    };
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Couldn't start the C# helper ({}): {e}", helper.display()))?;
    let mut stdin = child.stdin.take().ok_or("The C# helper has no input.")?;
    let list: String = files.iter().map(|f| format!("{}\n", f.to_string_lossy())).collect();
    // Written from its own thread, so a long list can't stall both sides.
    let writer = std::thread::spawn(move || stdin.write_all(list.as_bytes()));
    let mut readings = HashMap::new();
    for line in BufReader::new(child.stdout.take().ok_or("The C# helper has no output.")?).lines() {
        let line = line.map_err(|e| format!("Couldn't read the C# helper's answer: {e}"))?;
        let v: Value =
            serde_json::from_str(&line).map_err(|e| format!("The C# helper wrote something odd ({e}): {line}"))?;
        if let Some(file) = v.get("file").and_then(Value::as_str) {
            readings.insert(file.to_owned(), v);
        }
    }
    let _ = writer.join();
    let status = child.wait().map_err(|e| e.to_string())?;
    if !status.success() {
        return Err(format!("The C# helper stopped with {status}."));
    }
    Ok(readings)
}
