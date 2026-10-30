//! Compares what the two apps found in a file, field by field.

use serde_json::Value;

/// One place where the two differ: where (`info.samples[2]`), and what each app found there.
#[derive(Debug, PartialEq)]
pub struct Difference {
    pub path: String,
    pub cs: String,
    pub rust: String,
    /// The place with the list positions and map keys taken out: see [`Difference::kind`].
    kind: String,
}

impl Difference {
    /// The place with the list positions and map keys taken out (`song.channels[].name`, `info.unplayed{}`), so the
    /// same kind of difference in many files counts together. It holds no names or notes, so it's safe to paste into a
    /// conversation: it's made as the readings are walked, so a sample named "Kick [808].wav" can't show through.
    pub fn kind(&self) -> String {
        self.kind.clone()
    }
}

/// Numbers this close count as the same: the two apps print floats a little differently.
const CLOSE: f64 = 1e-6;

/// Every difference between the C# app's reading and the Rust one's. Objects' keys are written into the path as
/// fields (`info.title`), except in the maps keyed by a sample's path or a channel (`info.unplayed{C:\Kick.wav}`),
/// whose keys [`Difference::kind`] leaves out.
pub fn differences(cs: &Value, rust: &Value) -> Vec<Difference> {
    let mut out = Vec::new();
    walk(&Place::default(), cs, rust, &mut out);
    out
}

/// Where the walk is: the path, and the same with positions and map keys taken out.
#[derive(Clone, Default)]
struct Place {
    path: String,
    kind: String,
}

impl Place {
    fn then(&self, path: &str, kind: &str) -> Place {
        Place { path: format!("{}{path}", self.path), kind: format!("{}{kind}", self.kind) }
    }

    fn differs(&self, cs: String, rust: String) -> Difference {
        Difference { path: self.path.clone(), cs, rust, kind: self.kind.clone() }
    }
}

fn walk(at: &Place, cs: &Value, rust: &Value, out: &mut Vec<Difference>) {
    match (cs, rust) {
        (Value::Object(a), Value::Object(b)) => {
            // Maps keyed by a song's own sample paths or channels.
            let named = matches!(at.kind.rsplit('.').next(), Some("unplayed" | "curves"));
            let mut keys: Vec<&String> = a.keys().chain(b.keys().filter(|k| !a.contains_key(*k))).collect();
            keys.sort();
            for key in keys {
                let here = if named {
                    at.then(&format!("{{{key}}}"), "{}")
                } else if at.path.is_empty() {
                    at.then(key, key)
                } else {
                    at.then(&format!(".{key}"), &format!(".{key}"))
                };
                match (a.get(key), b.get(key)) {
                    (Some(x), Some(y)) => walk(&here, x, y, out),
                    (x, y) => out.push(here.differs(shown(x), shown(y))),
                }
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            for (i, (x, y)) in a.iter().zip(b).enumerate() {
                walk(&at.then(&format!("[{i}]"), "[]"), x, y, out);
            }
            if a.len() != b.len() {
                // What the longer list has beyond the other is part of what's compared, so a change to it shows.
                let extra = |list: &[Value]| {
                    let more: Vec<String> = list.iter().skip(a.len().min(b.len())).map(Value::to_string).collect();
                    if more.is_empty() { list.len().to_string() } else { format!("{} {}", list.len(), more.join(" ")) }
                };
                out.push(at.then(".len", ".len").differs(extra(a), extra(b)));
            }
        }
        (Value::Number(a), Value::Number(b)) => {
            // Whole numbers (ticks, ids, counts) must match exactly; only fractions get the tolerance. (C# writes a
            // fraction that happens to be whole, such as a tempo of 140, without its ".0": that one is compared as a
            // fraction, since the Rust side writes it with one.)
            let whole = |n: &serde_json::Number| n.as_i64().map(i128::from).or(n.as_u64().map(i128::from));
            let differ = match (whole(a), whole(b)) {
                (Some(x), Some(y)) => x != y,
                _ => {
                    let (x, y) = (a.as_f64().unwrap_or(f64::NAN), b.as_f64().unwrap_or(f64::NAN));
                    (x - y).abs() > CLOSE * x.abs().max(1.0)
                }
            };
            if differ {
                out.push(at.differs(cs.to_string(), rust.to_string()));
            }
        }
        _ if cs != rust => out.push(at.differs(cs.to_string(), rust.to_string())),
        _ => {}
    }
}

/// What one side has at a key the other side doesn't.
fn shown(v: Option<&Value>) -> String {
    v.map_or_else(|| "(none)".into(), Value::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn same_readings_have_no_differences() {
        let v = json!({ "info": { "tempo": 140.0, "samples": ["a", "b"] } });
        assert_eq!(differences(&v, &v), []);
    }

    #[test]
    fn numbers_within_a_hair_are_the_same() {
        assert_eq!(differences(&json!({ "pan": 0.5 }), &json!({ "pan": 0.500_000_000_1 })), []);
        assert_eq!(differences(&json!({ "tempo": 140 }), &json!({ "tempo": 140.0 })), []);
    }

    #[test]
    fn whole_numbers_must_match_exactly() {
        assert_eq!(differences(&json!({ "start": 1_152_000 }), &json!({ "start": 1_152_001 })).len(), 1);
        assert_eq!(differences(&json!({ "id": 3_000_000_000u64 }), &json!({ "id": 3_000_001_000u64 })).len(), 1);
        assert_eq!(differences(&json!({ "id": 3_000_000_000u64 }), &json!({ "id": -1 })).len(), 1);
    }

    #[test]
    fn lists_where_they_differ() {
        let cs = json!({ "song": { "channels": [{ "name": "Kick" }, { "name": "Snare" }] } });
        let rust = json!({ "song": { "channels": [{ "name": "Kick" }, { "name": "Hat" }, { "name": "Pad" }] } });
        let d = differences(&cs, &rust);
        assert_eq!(
            d.iter().map(|d| d.path.as_str()).collect::<Vec<_>>(),
            ["song.channels[1].name", "song.channels.len"]
        );
        assert_eq!(d[0].kind(), "song.channels[].name");
        // The extra channel is part of it, so a change there shows.
        assert_eq!((d[1].cs.as_str(), d[1].rust.as_str()), ("2", r#"3 {"name":"Pad"}"#));
    }

    #[test]
    fn a_map_of_samples_keeps_their_names_out_of_the_path() {
        let cs = json!({ "info": { "unplayed": { "C:\\Kick.wav": "NoNotes" } } });
        let rust = json!({ "info": { "unplayed": {} } });
        let d = differences(&cs, &rust);
        assert_eq!(d[0].path, "info.unplayed{C:\\Kick.wav}");
        assert_eq!(d[0].kind(), "info.unplayed{}");
        assert_eq!(d[0].cs, "\"NoNotes\"");
        assert_eq!(d[0].rust, "(none)");
    }

    #[test]
    fn a_sample_named_with_brackets_stays_out_of_the_kind() {
        let cs = json!({ "info": { "unplayed": { "C:\\Samples\\Kick [808] {x}.wav": "NoNotes" } } });
        let rust = json!({ "info": { "unplayed": { "C:\\Samples\\Kick [808] {x}.wav": "Muted" } } });
        let d = differences(&cs, &rust);
        assert_eq!(d[0].kind(), "info.unplayed{}");
    }
}
