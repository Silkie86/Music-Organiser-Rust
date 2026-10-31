//! Spare copies (Solomon's F12 note, 25 Oct 2026: "if i have lots of aax plugins but i never use them or have a vst 2
//! version i never use show that"). The C# app's `SpareCopies`.
//!
//! Plugin installers often put the same plugin in every format: VST2, VST3, CLAP and AAX. A copy is spare when no
//! project uses that format and a format they do use stays installed; AAX copies are spare when Pro Tools, the only
//! DAW that loads them, isn't on the computer. When a project's format isn't known ("VST" from an old FL Studio save),
//! every copy of that plugin counts as used. Nothing here removes anything.

use std::collections::{BTreeMap, HashMap};

use mo_formats::case;

use crate::plugins::{InstalledPlugin, family, is_waves};

/// A plugin file in a format none of your projects use, while the format they do use is installed too.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpareCopy {
    pub name: String,
    pub format: String,
    pub path: String,
    pub bytes: u64,
    pub why: Why,
}

/// Why a copy is spare.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Why {
    /// Only Pro Tools loads AAX, and it isn't on this computer.
    NoProTools,
    /// Your projects use these formats of it (with how many projects use each), and none use this one.
    OtherFormatsUsed { used: Vec<String>, projects: u32 },
}

impl SpareCopy {
    /// What the app says, as the C# app says it.
    pub fn why_text(&self) -> String {
        match &self.why {
            Why::NoProTools => "Only Pro Tools loads AAX, and it isn't on this computer.".to_owned(),
            Why::OtherFormatsUsed { used, projects } => {
                let n = if *projects == 1 { "1 project".to_owned() } else { format!("{projects} projects") };
                format!("Your projects use its {} ({n}); none use this {}.", used.join(" and "), self.format)
            }
        }
    }
}

/// The formats a project can name that say which file it loads.
const KNOWN: [&str; 4] = ["VST2", "VST3", "CLAP", "AU"];

/// Finds the spare copies among `files` (every plugin file found, AAX ones included). `used` is each plugin name your
/// projects use, with how many projects use each format; `pro_tools` says whether Pro Tools is installed; `size` says
/// how big a file or bundle is (only asked for the spare ones). Biggest first.
pub fn find(
    files: &[InstalledPlugin],
    used: &HashMap<String, HashMap<String, u32>>,
    pro_tools: bool,
    size: impl Fn(&str) -> u64,
) -> Vec<SpareCopy> {
    // Each used name by its family ("FabFilter Pro-Q 3" and "Pro-Q 3" are both families of the files' names).
    let used_families: Vec<(String, &HashMap<String, u32>)> =
        used.iter().map(|(name, formats)| (family(name), formats)).filter(|(f, _)| f.chars().count() >= 2).collect();
    // The files by family, in the order their families first come.
    let mut groups: Vec<(String, Vec<&InstalledPlugin>)> = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    for f in files.iter().filter(|f| !is_waves(&f.name, &f.path)) {
        let fam = family(&f.name);
        match at.get(&fam) {
            Some(&i) => {
                if let Some((_, g)) = groups.get_mut(i) {
                    g.push(f);
                }
            }
            None => {
                at.insert(fam.clone(), groups.len());
                groups.push((fam, vec![f]));
            }
        }
    }
    let mut spare = Vec::new();
    for (fam, copies) in groups {
        if fam.chars().count() < 2 {
            continue;
        }
        // Formats by their name in capitals, so "vst3" and "VST3" are one.
        let mut formats: BTreeMap<String, u32> = BTreeMap::new();
        for (_, f) in used_families.iter().filter(|(u, _)| close(u, &fam)) {
            for (format, n) in f.iter() {
                let entry = formats.entry(case::key(format)).or_insert(0);
                *entry = entry.saturating_add(*n);
            }
        }
        let unknown = formats.keys().any(|f| !KNOWN.contains(&f.as_str()));
        for copy in &copies {
            let format = case::key(&copy.format);
            if format == "AAX" {
                if !pro_tools {
                    spare.push(SpareCopy {
                        name: copy.name.clone(),
                        format: "AAX".to_owned(),
                        path: copy.path.clone(),
                        bytes: size(&copy.path),
                        why: Why::NoProTools,
                    });
                }
                continue;
            }
            // Never used at all: the Never used chip has it, whole. A format no one knows: keep everything.
            if formats.is_empty() || unknown || formats.contains_key(&format) {
                continue;
            }
            // The used formats that are installed, in the order the copies come.
            let mut kept: Vec<String> = Vec::new();
            for c in &copies {
                let k = case::key(&c.format);
                if formats.contains_key(&k) && !kept.iter().any(|x| case::key(x) == k) {
                    kept.push(c.format.clone());
                }
            }
            // What the projects use isn't installed: this copy may be the next best thing.
            if kept.is_empty() {
                continue;
            }
            let projects = kept.iter().map(|f| formats.get(&case::key(f)).copied().unwrap_or(0)).sum();
            spare.push(SpareCopy {
                name: copy.name.clone(),
                format: copy.format.clone(),
                path: copy.path.clone(),
                bytes: size(&copy.path),
                why: Why::OtherFormatsUsed { used: kept, projects },
            });
        }
    }
    spare.sort_by_key(|s| std::cmp::Reverse(s.bytes));
    spare
}

/// Whether two families are the same plugin: equal, or one with its maker in front of the other ("fabfilterproq" and
/// "proq") when both are at least four letters. C# took one anywhere inside the other, which gave "Serum FX" Serum's
/// projects.
fn close(a: &str, b: &str) -> bool {
    a == b || (a.chars().count() >= 4 && b.chars().count() >= 4 && (a.ends_with(b) || b.ends_with(a)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, format: &str) -> InstalledPlugin {
        InstalledPlugin { name: name.into(), format: format.into(), path: format!("C:/Plugins/{name}.{format}") }
    }

    fn used(list: &[(&str, &[(&str, u32)])]) -> HashMap<String, HashMap<String, u32>> {
        list.iter()
            .map(|(name, formats)| (name.to_string(), formats.iter().map(|(f, n)| (f.to_string(), *n)).collect()))
            .collect()
    }

    #[test]
    fn a_copy_in_a_format_no_project_uses_is_spare_while_the_used_one_is_installed() {
        let files = [
            file("Serum_x64", "VST2"),
            file("Serum", "VST3"),
            file("Serum", "AAX"),
            file("Vital", "CLAP"),
            file("Vital", "VST3"),
            file("Diva", "VST2"),
            file("Diva", "VST3"),
            file("Waves L2", "VST3"),
        ];
        let used = used(&[("Serum", &[("VST3", 4)]), ("Vital", &[("VST", 2)]), ("Diva(x64)", &[("vst3", 1)])]);
        let spare = find(&files, &used, false, |p| p.len() as u64);
        let lines: Vec<(String, String, String)> =
            spare.iter().map(|s| (s.name.clone(), s.format.clone(), s.why_text())).collect();
        assert_eq!(
            lines,
            [
                (
                    "Serum_x64".into(),
                    "VST2".into(),
                    "Your projects use its VST3 (4 projects); none use this VST2.".into()
                ),
                // Ties keep the order the files came in.
                ("Serum".into(), "AAX".into(), "Only Pro Tools loads AAX, and it isn't on this computer.".into()),
                ("Diva".into(), "VST2".into(), "Your projects use its VST3 (1 project); none use this VST2.".into()),
            ]
        );
        // With Pro Tools, AAX copies are in use.
        assert_eq!(find(&files, &used, true, |_| 0).len(), 2);
    }

    #[test]
    fn a_plugin_named_inside_another_isnt_that_one() {
        let files =
            [file("Serum FX", "VST2"), file("Serum FX", "VST3"), file("Pro-Q 3", "VST2"), file("Pro-Q 3", "VST3")];
        let used = used(&[("Serum", &[("VST3", 4)]), ("FabFilter Pro-Q 3", &[("VST3", 2)])]);
        let spare = find(&files, &used, false, |_| 0);
        let names: Vec<(&str, &str)> = spare.iter().map(|s| (s.name.as_str(), s.format.as_str())).collect();
        assert_eq!(names, [("Pro-Q 3", "VST2")]);
    }

    #[test]
    fn a_used_format_that_isnt_installed_keeps_the_other_copies() {
        let files = [file("Massive", "VST2")];
        let used = used(&[("Massive", &[("VST3", 3)])]);
        assert!(find(&files, &used, false, |_| 0).is_empty());
    }
}
