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

const TYPES: [&str; 4] = ["Kick", "Closed hat", "Snare", "808"];

/// The library: 160 sounds, 40 of each type around a point of its own, in packs dated 2018 to 2023 (mid-year, so
/// the time zone can't move one into another year). With `clap`, deep listening has heard them all.
fn library(seed: u64, clap: bool) -> (Vec<Value>, Vec<Value>) {
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
            format!("/samples/Pack {pack}/{}/{} {i:03}.wav", TYPES[t], TYPES[t])
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

fn queries() -> Vec<Options> {
    let o = Options::default();
    vec![
        Options { seed: 7, count: 10, ..o.clone() },
        Options { sound_type: Some("Kick".into()), count: 5, adventure: 0.0, seed: 3, ..o.clone() },
        Options { sound_type: Some("Closed hat".into()), count: 8, adventure: 0.6, seed: 11, max_uses: 1, ..o.clone() },
        Options { year: Some(2020), count: 6, seed: 5, ..o.clone() },
        Options { sound_type: Some("808".into()), count: 60, seed: 13, ..o.clone() },
        Options { sound_type: Some("Vocal".into()), seed: 2, ..o.clone() },
        Options { year: Some(1999), seed: 2, ..o.clone() },
        Options { sound_type: Some("Snare".into()), year: Some(2018), max_uses: 99, count: 4, seed: 17, ..o },
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

fn query_json(o: &Options) -> Value {
    json!({
        "Type": o.sound_type,
        "Count": o.count,
        "Adventure": o.adventure,
        "Seed": o.seed,
        "MaxUses": o.max_uses,
        "Year": o.year,
    })
}

/// What the Rust app answers, read from the sounds.db the C# app saved.
fn rust_side(db: &Path, uses: &[Value]) -> Result<Value, String> {
    let db = SoundsDb::open_read_only(db).map_err(|e| format!("Rust couldn't read the sounds.db C# saved: {e}"))?;
    let sounds = db.all(Local::now().naive_local()).map_err(|e| e.to_string())?;
    let uses: Vec<SampleUse> = uses
        .iter()
        .map(|u| SampleUse {
            path: u[0].as_str().unwrap_or_default().to_owned(),
            projects: u[1].as_u64().unwrap_or(0) as u32,
            recent: u[1].as_u64().unwrap_or(0) as u32,
            years: Vec::new(),
        })
        .collect();
    let usage = usage(&sounds, &uses);
    let index = SoundIndex::read(&db, &sounds).map_err(|e| e.to_string())?;
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
    let names: serde_json::Map<String, Value> = sounds.iter().map(|s| (s.id.to_string(), json!(s.name()))).collect();
    Ok(json!({
        "names": names,
        "answers": answers,
        "kits": kits,
        "years": years(&sounds),
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

fn short(v: &Value) -> String {
    let s = v.to_string();
    if s.len() > 400 { format!("{}…", &s[..s.floor_char_boundary(400)]) } else { s }
}

/// Writes the libraries into `dir` (the twin's own new folder), has the C# app save and answer, reads what it saved
/// with the Rust app, and gives every difference.
pub fn compare(helper: &Path, dir: &Path) -> Result<Vec<String>, String> {
    let cases = [("features", library(101, false)), ("deep listening", library(202, true))];
    let mut files = Vec::new();
    for (i, (_, (sounds, uses))) in cases.iter().enumerate() {
        let path = dir.join(format!("rediscover-{i}.json"));
        let body = json!({
            "Db": dir.join(format!("rediscover-{i}.db")),
            "Sounds": sounds,
            "Usage": uses,
            "Queries": queries().iter().map(query_json).collect::<Vec<_>>(),
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
            if !same(&a["WhyNothing"], &b["WhyNothing"]) {
                differences.push(format!(
                    "Rediscover, {name}, question {q}: C# says {}, Rust {}",
                    a["WhyNothing"], b["WhyNothing"]
                ));
            }
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
}
