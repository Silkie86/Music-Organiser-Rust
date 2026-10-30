//! The `formatlab` command: Format Lab and the census from a terminal.

use std::fmt::Write as _;
use std::io::Write;
use std::path::PathBuf;

use crate::census::{self, Census};
use crate::lab::{LabFile, count};
use crate::ops;
use crate::registry::{self, Registry};

pub const USAGE: &str = "\
Format Lab: looks inside Bitwig and FL Studio files, to learn how each DAW saves things.

  formatlab tree <file> [--depth N]        everything in the file, named where the app knows the name
  formatlab diff <before> <after>          every value that differs between two saves
  formatlab find <file> <value> [--text]   everywhere the file holds a value: text or a number
  formatlab census <folder>... [--top N]   what the app doesn't know yet, across all projects in folders
  formatlab fields bitwig|fl [<id>]        what the app knows (the field registry)

Files: .bwproject, .bwpreset, .bwclip and .flp.
";

/// How many finds are listed.
const FINDS_SHOWN: usize = 200;

/// Runs one command; returns the exit code (0 done, 1 failed, 2 not understood).
pub fn run(args: &[String], out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let Some(command) = args.first() else {
        let _ = writeln!(out, "{USAGE}");
        return 2;
    };
    if matches!(command.as_str(), "help" | "-h" | "--help" | "/?") {
        let _ = writeln!(out, "{USAGE}");
        return 0;
    }
    let mut rest: Vec<String> = args.get(1..).unwrap_or_default().to_vec();
    let mut option = |name: &str| -> Option<String> {
        let i = rest.iter().position(|a| a == name)?;
        rest.remove(i);
        Some(if i < rest.len() { rest.remove(i) } else { String::new() })
    };
    let number = |text: Option<String>, fallback: usize| {
        text.filter(|t| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|t| t.parse().ok())
            .unwrap_or(fallback)
    };

    let result = match command.as_str() {
        "tree" => {
            let depth = number(option("--depth"), usize::MAX);
            let [file] = rest.as_slice() else { return wrong(err, "tree needs one file") };
            LabFile::open(file.as_ref()).map(|f| ops::tree(&f, depth))
        }
        "diff" => {
            let [a, b] = rest.as_slice() else {
                return wrong(err, "diff needs two files: the save before and the save after");
            };
            LabFile::open(a.as_ref()).and_then(|a| {
                let b = LabFile::open(b.as_ref())?;
                if a.format != b.format {
                    return Err("Those are files from two different DAWs.".to_owned());
                }
                Ok(ops::diff_text(&a, &b, &ops::diff(&a, &b)))
            })
        }
        "find" => {
            let text = match rest.iter().position(|a| a == "--text") {
                Some(i) => {
                    rest.remove(i);
                    true
                }
                None => false,
            };
            let [file, value] = rest.as_slice() else { return wrong(err, "find needs a file and a value") };
            LabFile::open(file.as_ref()).map(|f| ops::find_text(&f, value, &ops::find(&f, value, text), FINDS_SHOWN))
        }
        "census" => {
            let top = number(option("--top"), 40);
            if rest.is_empty() {
                return wrong(err, "census needs at least one folder");
            }
            let files = census::find_projects(&rest.iter().map(PathBuf::from).collect::<Vec<_>>());
            let _ = writeln!(err, "Counting {} projects…", count(files.len()));
            // Progress goes to the terminal straight away, not through `err`, which another thread can't share.
            let census = Census::run(&files, |n| {
                if n.is_multiple_of(1000) && n > 0 {
                    eprintln!("  {}", count(n));
                }
            });
            Ok(census.report(top))
        }
        "fields" => {
            let (which, id) = match rest.as_slice() {
                [w] => (w.as_str(), None),
                [w, id] => (w.as_str(), Some(id.as_str())),
                _ => return wrong(err, "fields needs bitwig or fl"),
            };
            let r = match which {
                "bitwig" => registry::bitwig(),
                "fl" => registry::fl_studio(),
                _ => return wrong(err, "fields needs bitwig or fl"),
            };
            Ok(fields(r, id))
        }
        other => return wrong(err, &format!("there's no command \"{other}\"")),
    };
    match result {
        Ok(text) => {
            let _ = out.write_all(text.as_bytes());
            0
        }
        Err(why) => {
            let _ = writeln!(err, "Format Lab: {why}");
            1
        }
    }
}

/// What the registry knows: every entry, or everything about the ones an id or a word in the name picks out.
pub fn fields(r: &Registry, id: Option<&str>) -> String {
    let mut out = String::new();
    if id.is_none() {
        let _ = writeln!(out, "{}", r.format);
        let _ = writeln!(out, "{}", r.about);
    }
    let groups = [
        ("classes", &r.classes),
        ("fields", &r.fields),
        ("metadata", &r.meta),
        ("events", &r.events),
        ("Fruity Wrapper records", &r.wrapper),
        ("VST3 records", &r.vst3),
    ];
    let wanted = id.map(str::to_lowercase);
    for (name, entries) in groups {
        let shown: Vec<_> = entries
            .iter()
            .filter(|e| match &wanted {
                None => true,
                Some(w) => e.id.to_lowercase() == *w || e.name.to_lowercase().contains(w.as_str()),
            })
            .collect();
        if shown.is_empty() {
            continue;
        }
        let _ = writeln!(out);
        let _ = writeln!(out, "{name}:");
        for e in shown {
            let kind = e.kind.as_deref().map(|k| format!(" ({k})")).unwrap_or_default();
            let _ = writeln!(out, "  {:<8} {}{kind}", e.id, e.name);
            if id.is_none() {
                continue;
            }
            let more = [
                ("", Some(&e.about).filter(|a| !a.is_empty())),
                ("layout: ", e.layout.as_ref()),
                ("each record: ", e.record_layout.as_ref()),
                ("seen in: ", e.seen.as_ref()),
                ("how we know: ", e.proof.as_ref()),
                ("in the code: ", e.code.as_ref()),
            ];
            for (label, text) in more {
                if let Some(t) = text {
                    let _ = writeln!(out, "           {label}{t}");
                }
            }
        }
    }
    out
}

fn wrong(err: &mut dyn Write, why: &str) -> i32 {
    let _ = writeln!(err, "Format Lab: {why}.");
    let _ = writeln!(err);
    let _ = writeln!(err, "{USAGE}");
    2
}
