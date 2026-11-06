//! The Rediscover and kits twin: a made-up sound library (four types in clusters, some used in projects, starred, rated,
//! played or hidden) is saved by the C# app's own sound library to a new sounds.db, which the Rust side then reads as
//! it reads the real one. Both apps answer the same Rediscover questions (seeded, so the "chance" part is the same)
//! and must suggest the same sounds in the same order, for the same reasons; then both roll the same seeded kits and
//! must fill the same pads with the same sounds. (A described vibe needs deep listening's model, so it isn't checked.) Run as part of `twin sounds`, in its own
//! folder: the sounds.db is made there and removed with it.

use std::path::Path;

use chrono::Local;
use mo_library::SampleUse;
use mo_sounds::SoundsDb;
use mo_sounds::index::SoundIndex;
use mo_sounds::kits::{Options as KitOptions, default_drum_kit, roll};
use mo_sounds::rediscover::{Options, folders, suggest, why_nothing, years};
use mo_sounds::usage::usage;
use serde_json::{Value, json};

use crate::sounds::same;

/// Numbers from a fixed seed: 0 up to 1.
struct Numbers(u64);

impl Numbers {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }
}

const RATES: [i32; 3] = [44_100, 48_000, 96_000];

const TYPES: [&str; 4] = ["Kick", "Closed hat", "Snare", "808"];

/// The library: 160 sounds, 40 of each type around a point of its own, in packs dated 2018 to 2023 (mid-year, so
/// the time zone can't move one into another year). With `clap`, deep listening has heard them all.
fn library(seed: u64, clap: bool) -> (Vec<Value>, Vec<Value>) {
    let ext = |i: usize| {
        if i % 6 == 5 {
            "mp3"
        } else if i % 11 == 9 {
            "flac"
        } else {
            "wav"
        }
    };
    let mut n = Numbers(seed | 1);
    let centres: Vec<Vec<f32>> = TYPES.iter().map(|_| (0..20).map(|_| n.next() * 4.0).collect()).collect();
    let clap_centres: Vec<Vec<f32>> = TYPES.iter().map(|_| (0..16).map(|_| n.next() - 0.5).collect()).collect();
    let mut sounds = Vec::new();
    let mut uses = Vec::new();
    for i in 0..160usize {
        let t = i % 4;
        let pack = i / 40;
        // Two snares in different packs share a file name: a project naming just "shared.wav" credits neither.
        let path = if i == 10 || i == 50 {
            format!("/samples/Pack {pack}/{}/shared.wav", TYPES[t])
        } else {
            format!("/samples/Pack {pack}/{}/{} {i:03}.{}", TYPES[t], TYPES[t], ext(i))
        };
        // 2018-07-01 12:00 UTC plus a year a pack.
        let mtime = 1_530_446_400 + pack as i64 * 31_557_600;
        let features: Vec<f32> = centres[t].iter().map(|c| c + (n.next() - 0.5) * 3.0).collect();
        let vector: Vec<f32> = clap_centres[t].iter().map(|c| c + (n.next() - 0.5) * 0.6).collect();
        sounds.push(json!({
            "Path": path,
            "Mtime": mtime,
            "Features": features,
            "Clap": if clap { json!(vector) } else { Value::Null },
            "NameType": TYPES[t],
            // 808s have a pitch (for kits tuned to a key); a few sounds are loops (kits leave them out).
            "Pitch": if TYPES[t] == "808" { json!(29.0 + (i % 17) as f64 * 0.75) } else { Value::Null },
            "IsLoop": i % 29 == 11,
            // For the duplicates: lengths (two long ones, past what the exact hash covers), rates, levels, sizes.
            "Seconds": if i == 16 || i == 17 { 25.0 } else { 0.3 + (i % 13) as f64 * 0.4 },
            "SampleRate": RATES[i % 3],
            "PeakDb": -((i % 8) as f64) * 0.7,
            "Size": 1000 + i * 37,
            "Favourite": i % 17 == 3,
            "Rating": if i % 13 == 5 { 5 } else if i % 11 == 2 { 2 } else { 0 },
            "Plays": if i % 9 == 4 { 4 } else { (i % 5 == 1) as i32 },
            "Hidden": i % 23 == 7,
        }));
        // Some used by full path, some by file name alone (only a name no other sound has counts).
        if i % 7 == 0 {
            uses.push(json!([path, 1 + (i % 3) as i32]));
        } else if i % 19 == 6 {
            uses.push(json!([format!("{} {i:03}.wav", TYPES[t]), 2]));
        }
    }
    uses.push(json!(["shared.wav", 4]));
    (sounds, uses)
}

/// What the duplicate finder found in the library: audio hashes several sounds share (and a few no other has), and
/// pairs of copies of every kind, near and far apart, some joining one cluster to the next.
fn copies(sounds: &[Value]) -> (Vec<Value>, Vec<Value>) {
    let path = |i: usize| sounds[i]["Path"].clone();
    let kinds = ["Exact", "Converted", "Trimmed"];
    let aparts = [json!(0.05), json!(0.5), json!(1.0), json!(2.0), Value::Null];
    let typicals = [Value::Null, json!(0.5), json!(2.0), json!(4.0)];
    let (mut prints, mut matches) = (Vec::new(), Vec::new());
    for k in 0..20usize {
        let base = k * 8;
        let hash = format!("{:02X}AB{k:02X}", k * 7);
        prints.push(json!([path(base), hash]));
        prints.push(json!([path(base + 1), hash]));
        if k % 3 == 0 {
            prints.push(json!([path(base + 2), hash]));
        }
        prints.push(json!([path(base + 7), format!("FF{k:02X}")]));
        let m = |a: usize, b: usize, kind: &str, apart: Value, typical: Value| {
            json!([path(a), path(b), kind, 0.9, 0.0, apart, typical])
        };
        matches.push(m(base, base + 3, kinds[k % 3], aparts[k % 5].clone(), typicals[k % 4].clone()));
        matches.push(m(base + 3, base + 4, "Converted", json!(0.08), Value::Null));
        if k % 2 == 0 {
            matches.push(m(base + 4, base + 5, "Trimmed", json!(0.3), json!(1.0)));
        }
        matches.push(m(base + 1, base + 6, "Converted", json!(0.9), json!(0.2)));
        if k % 5 == 4 && k < 19 {
            matches.push(m(base + 5, base + 13, "Converted", json!(0.02), Value::Null));
        }
    }
    (prints, matches)
}

fn queries() -> Vec<Options> {
    let o = Options::default();
    vec![
        Options { seed: 7, count: 10, ..o.clone() },
        Options { sound_types: vec!["Kick".into()], count: 5, adventure: 0.0, seed: 3, ..o.clone() },
        Options {
            sound_types: vec!["Closed hat".into()],
            count: 8,
            adventure: 0.6,
            seed: 11,
            max_uses: 1,
            ..o.clone()
        },
        Options { years: Some((2020, 2020)), count: 6, seed: 5, ..o.clone() },
        Options { sound_types: vec!["808".into()], count: 60, seed: 13, ..o.clone() },
        Options { sound_types: vec!["Vocal".into()], seed: 2, ..o.clone() },
        Options { years: Some((1999, 1999)), seed: 2, ..o.clone() },
        Options { sound_types: vec!["Snare".into()], years: Some((2018, 2018)), max_uses: 99, count: 4, seed: 17, ..o },
    ]
}

/// Kits to roll: whether to prefer unused sounds, one pack, the key (0 = C), the seed, how random, and a sound to lock
/// on the kick pad (by its path).
struct Kit {
    options: KitOptions,
    lock_kick: Option<&'static str>,
}

fn kits() -> Vec<Kit> {
    let o = KitOptions::default();
    vec![
        Kit { options: KitOptions { seed: 21, ..o.clone() }, lock_kick: None },
        Kit { options: KitOptions { seed: 22, randomness: 0.0, prefer_unused: false, ..o.clone() }, lock_kick: None },
        Kit { options: KitOptions { seed: 23, same_pack: true, randomness: 1.0, ..o.clone() }, lock_kick: None },
        Kit {
            options: KitOptions { seed: 24, same_pack: true, key_tonic: Some(5), ..o.clone() },
            lock_kick: Some("/samples/Pack 2/Kick/Kick 084.wav"),
        },
        Kit { options: KitOptions { seed: 25, key_tonic: Some(0), randomness: 0.1, ..o }, lock_kick: None },
    ]
}

fn kit_json(k: &Kit) -> Value {
    json!({
        "PreferUnused": k.options.prefer_unused,
        "SamePack": k.options.same_pack,
        "KeyTonic": k.options.key_tonic,
        "Seed": k.options.seed,
        "Randomness": k.options.randomness,
        "LockKick": k.lock_kick,
    })
}

/// A question as the C# app takes it: one type and one year at most. One asking for more is refused, not cut down to
/// what C# takes, which would ask the two apps different questions (review of 7cc595f).
fn query_json(o: &Options) -> Result<Value, String> {
    if o.sound_types.len() > 1 {
        return Err(format!("The C# app picks one type at most, so this can't be asked of it: {o:?}"));
    }
    if o.years.is_some_and(|(first, last)| first != last) {
        return Err(format!("The C# app picks one year at most, so this can't be asked of it: {o:?}"));
    }
    Ok(json!({
        "Type": o.sound_types.first(),
        "Count": o.count,
        "Adventure": o.adventure,
        "Seed": o.seed,
        "MaxUses": o.max_uses,
        // A span of one year on the Rust app's strip.
        "Year": o.years.map(|(first, _)| first),
    }))
}

/// What the Rust app answers, read from the sounds.db the C# app saved.
fn rust_side(db: &Path, uses: &[Value]) -> Result<Value, String> {
    let db = SoundsDb::open_read_only(db).map_err(|e| format!("Rust couldn't read the sounds.db C# saved: {e}"))?;
    // As the Sounds page reads it: the sounds and their vectors in one pass.
    let everything = db.everything(Local::now().naive_local()).map_err(|e| e.to_string())?;
    let analysed = everything.analysed();
    let index = SoundIndex::from_gathered(everything.features, everything.clap, analysed);
    let sounds = everything.sounds;
    let uses: Vec<SampleUse> = uses
        .iter()
        .map(|u| SampleUse {
            path: u[0].as_str().unwrap_or_default().to_owned(),
            projects: u[1].as_u64().unwrap_or(0) as u32,
            recent: u[1].as_u64().unwrap_or(0) as u32,
            years: Vec::new(),
            project_ids: Vec::new(),
        })
        .collect();
    let usage = usage(&sounds, &uses);
    let answers: Vec<Value> = queries()
        .iter()
        .map(|o| {
            let found = suggest(&sounds, &usage, &index, o);
            json!({
                "Suggestions": found.iter().map(|s| json!({ "Id": s.id, "Reason": s.reason, "Score": s.score })).collect::<Vec<_>>(),
                "WhyNothing": if found.is_empty() { json!(why_nothing(&sounds, &index, o)) } else { Value::Null },
            })
        })
        .collect();
    let kits: Vec<Value> = kits()
        .iter()
        .map(|k| {
            let mut slots = default_drum_kit();
            if let Some(path) = k.lock_kick {
                slots[0].sound = sounds.iter().find(|s| s.path == path).map(|s| s.id);
                slots[0].locked = true;
            }
            let seed = roll(&sounds, &usage, &index, &mut slots, &k.options);
            json!({ "Seed": seed, "Pads": slots.iter().map(|s| s.sound).collect::<Vec<_>>() })
        })
        .collect();
    let duplicates: Vec<Value> = mo_sounds::duplicates::read(&db, &sounds, &usage)
        .map_err(|e| e.to_string())?
        .iter()
        .map(|g| {
            json!({
                "Kind": format!("{:?}", g.kind),
                "SpareBytes": g.spare_bytes,
                "Members": g.members.iter().map(|m| json!({
                    "Id": m.id,
                    "Size": m.size,
                    "Keep": m.keep,
                    "Kind": m.kind.map(|k| format!("{k:?}")),
                    "Why": m.why,
                    "Same": m.same,
                    "SameSet": m.same_set,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    let names: serde_json::Map<String, Value> = sounds.iter().map(|s| (s.id.to_string(), json!(s.name()))).collect();
    Ok(json!({
        "names": names,
        "answers": answers,
        "kits": kits,
        "duplicates": duplicates,
        // The years alone, newest first, less undated files, as the C# app listed them (the Rust strip also counts each
        // one's sounds, and leaves out years still to come).
        "years": years(&sounds).iter().map(|y| y.0).filter(|&y| y > 1980).collect::<Vec<_>>(),
        "folders": folders(&sounds, &usage)
            .iter()
            .map(|f| json!({ "Folder": f.folder, "Sounds": f.sounds, "Used": f.used, "Share": f.share }))
            .collect::<Vec<_>>(),
    }))
}

/// Suggestions match when the sounds, their order and their reasons are the same, and the scores (to 3 places) are
/// within one step: the features index's averages are added up in another order, so a score can round the other way.
/// One known difference (DIFFERENCES.md): C# can give a liked, unused sound as "like" itself; Rust names another.
fn same_suggestions(cs: &Value, rust: &Value, names: &Value) -> bool {
    let (Some(a), Some(b)) = (cs.as_array(), rust.as_array()) else { return false };
    let like_itself = |x: &Value| {
        let name = names[x["Id"].to_string()].as_str().unwrap_or("\u{0}");
        x["Reason"].as_str().is_some_and(|r| r.contains(&format!("Sounds like {name}, ")))
    };
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            x["Id"] == y["Id"]
                && (x["Reason"] == y["Reason"] || like_itself(x))
                && (x["Score"].as_f64().unwrap_or(f64::NAN) - y["Score"].as_f64().unwrap_or(0.0)).abs() < 0.0011
        })
}

/// C#'s words for no sounds in the year asked, as the Rust app says them (DIFFERENCES.md): Rust picks its years on the
/// year strip, so it says to pick others there.
fn year_words(why: &Value) -> Value {
    match why.as_str() {
        Some(w) => json!(w.replace("Pick another year, or Any year.", "Pick other years on the strip, or clear it.")),
        None => why.clone(),
    }
}

/// The duplicate groups less the one known difference (DIFFERENCES.md): a trimmed copy that's only like the others
/// was described against itself in C# (always "Longer version (x s)"), against the one to keep in Rust. Those C#
/// texts are left out; the Rust ones are still checked to be one of the two wordings.
fn known_differences(groups: &Value, rust: bool) -> Value {
    let mut g = groups.clone();
    for group in g.as_array_mut().into_iter().flatten() {
        for m in group["Members"].as_array_mut().into_iter().flatten() {
            if m["Same"] != json!(false) || m["Kind"] != json!("Trimmed") {
                continue;
            }
            let why = m["Why"].as_str().unwrap_or_default();
            let fits = if rust {
                (why.starts_with("Trimmed: ") && why.contains(" s of ") && why.ends_with(" s"))
                    || (why.starts_with("Longer version (") && why.ends_with(" s)"))
            } else {
                why.starts_with("Longer version (")
            };
            m["Why"] = if fits { Value::Null } else { json!(format!("unexpected: {why}")) };
        }
    }
    g
}

fn short(v: &Value) -> String {
    let s = v.to_string();
    if s.len() > 400 { format!("{}…", &s[..s.floor_char_boundary(400)]) } else { s }
}

/// Writes the libraries into `dir` (the twin's own new folder), has the C# app save and answer, reads what it saved
/// with the Rust app, and gives every difference.
pub fn compare(helper: &Path, dir: &Path) -> Result<Vec<String>, String> {
    let cases = [("features", library(101, false)), ("deep listening", library(202, true))];
    let questions = queries().iter().map(query_json).collect::<Result<Vec<_>, _>>()?;
    let mut files = Vec::new();
    for (i, (_, (sounds, uses))) in cases.iter().enumerate() {
        let path = dir.join(format!("rediscover-{i}.json"));
        let (prints, copies_found) = copies(sounds);
        let body = json!({
            "Db": dir.join(format!("rediscover-{i}.db")),
            "Sounds": sounds,
            "Usage": uses,
            "Prints": prints,
            "Matches": copies_found,
            "Queries": questions,
            "Kits": kits().iter().map(kit_json).collect::<Vec<_>>(),
        });
        std::fs::write(&path, body.to_string()).map_err(|e| format!("Couldn't write a Rediscover case: {e}"))?;
        files.push(path);
    }
    let cs = crate::run_helper_with(helper, &["--rediscover"], &files)?;
    let mut differences = Vec::new();
    for (i, ((name, (_, uses)), file)) in cases.iter().zip(&files).enumerate() {
        let Some(cs) = cs.get(file.to_string_lossy().as_ref()) else {
            differences.push(format!("Rediscover, {name}: no C# answer"));
            continue;
        };
        if let Some(e) = cs.get("error").or_else(|| cs.get("error_more")) {
            differences.push(format!("Rediscover, {name}: the C# helper failed: {e}"));
            continue;
        }
        let rust = rust_side(&dir.join(format!("rediscover-{i}.db")), uses)?;
        for (q, o) in queries().iter().enumerate() {
            let (a, b) = (&cs["answers"][q], &rust["answers"][q]);
            if !same_suggestions(&a["Suggestions"], &b["Suggestions"], &rust["names"]) {
                differences.push(format!(
                    "Rediscover, {name}, question {q} ({o:?}): C# {}, Rust {}",
                    short(&a["Suggestions"]),
                    short(&b["Suggestions"])
                ));
            }
            if !same(&a["WhyNothing"], &b["WhyNothing"]) && !same(&year_words(&a["WhyNothing"]), &b["WhyNothing"]) {
                differences.push(format!(
                    "Rediscover, {name}, question {q}: C# says {}, Rust {}",
                    a["WhyNothing"], b["WhyNothing"]
                ));
            }
        }
        if !same(&known_differences(&cs["duplicates"], false), &known_differences(&rust["duplicates"], true)) {
            differences.push(format!(
                "Duplicates, {name}: C# {}, Rust {}",
                short(&cs["duplicates"]),
                short(&rust["duplicates"])
            ));
        }
        for part in ["kits", "years", "folders"] {
            if !same(&cs[part], &rust[part]) {
                differences.push(format!(
                    "Rediscover, {name}: {part}: C# {}, Rust {}",
                    short(&cs[part]),
                    short(&rust[part])
                ));
            }
        }
    }
    Ok(differences)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_library_has_what_the_questions_need() {
        let (sounds, uses) = library(101, false);
        assert_eq!(sounds.len(), 160);
        assert!(sounds.iter().any(|s| s["Hidden"] == json!(true)));
        assert!(sounds.iter().any(|s| s["Favourite"] == json!(true)));
        assert!(uses.len() > 20);
    }

    #[test]
    fn every_question_is_one_the_c_sharp_app_can_take() {
        for o in queries() {
            assert!(query_json(&o).is_ok(), "{o:?}");
        }
        let two_types = Options { sound_types: vec!["Kick".into(), "Snare".into()], ..Options::default() };
        assert!(query_json(&two_types).is_err());
        let two_years = Options { years: Some((2019, 2020)), ..Options::default() };
        assert!(query_json(&two_years).is_err());
    }
}
