//! Compares what the two apps found in a file, field by field.

use serde_json::Value;

/// One place where the two differ: where (`info.samples[2]`), and what each app found there.
#[derive(Debug, PartialEq)]
pub struct Difference {
    pub path: String,
    pub cs: String,
    pub rust: String,
}

impl Difference {
    /// The place with the list positions taken out (`song.channels[].name`), so the same kind of difference in many
    /// files counts together. It holds no names or notes, so it's safe to paste into a conversation.
    pub fn kind(&self) -> String {
        let mut out = String::new();
        let mut in_brackets = false;
        for c in self.path.chars() {
            match c {
                '[' => {
                    in_brackets = true;
                    out.push('[');
                }
                ']' => {
                    in_brackets = false;
                    out.push(']');
                }
                _ if in_brackets => {}
                _ => out.push(c),
            }
        }
        out
    }
}

/// Numbers this close count as the same: the two apps print floats a little differently.
const CLOSE: f64 = 1e-6;

/// Every difference between the C# app's reading and the Rust one's. Objects' keys are written into the path as they
/// are, except the keys of maps keyed by a sample's path or a channel, which are written `{}` so the path stays free of
/// names.
pub fn differences(cs: &Value, rust: &Value) -> Vec<Difference> {
    let mut out = Vec::new();
    walk("", cs, rust, &mut out);
    out
}

fn walk(path: &str, cs: &Value, rust: &Value, out: &mut Vec<Difference>) {
    match (cs, rust) {
        (Value::Object(a), Value::Object(b)) => {
            // Maps whose keys are a song's own names (samples, channels, details) go in as `{}` with the key after it.
            let named = matches!(path.rsplit('.').next(), Some("unplayed" | "curves"));
            let mut keys: Vec<&String> = a.keys().chain(b.keys().filter(|k| !a.contains_key(*k))).collect();
            keys.sort();
            for key in keys {
                let here = if named {
                    format!("{path}{{}}")
                } else if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                match (a.get(key), b.get(key)) {
                    (Some(x), Some(y)) => walk(&here, x, y, out),
                    (x, y) => out.push(Difference { path: here, cs: shown(x, key, named), rust: shown(y, key, named) }),
                }
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            for (i, (x, y)) in a.iter().zip(b).enumerate() {
                walk(&format!("{path}[{i}]"), x, y, out);
            }
            if a.len() != b.len() {
                out.push(Difference {
                    path: format!("{path}.len"),
                    cs: a.len().to_string(),
                    rust: b.len().to_string(),
                });
            }
        }
        (Value::Number(a), Value::Number(b)) => {
            let (x, y) = (a.as_f64().unwrap_or(f64::NAN), b.as_f64().unwrap_or(f64::NAN));
            if (x - y).abs() > CLOSE * x.abs().max(1.0) {
                out.push(Difference { path: path.to_owned(), cs: cs.to_string(), rust: rust.to_string() });
            }
        }
        _ if cs != rust => out.push(Difference { path: path.to_owned(), cs: cs.to_string(), rust: rust.to_string() }),
        _ => {}
    }
}

/// What one side has at a key the other side doesn't.
fn shown(v: Option<&Value>, key: &str, named: bool) -> String {
    match v {
        None => "(none)".into(),
        Some(v) if named => format!("{key}: {v}"),
        Some(v) => v.to_string(),
    }
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
    fn lists_where_they_differ() {
        let cs = json!({ "song": { "channels": [{ "name": "Kick" }, { "name": "Snare" }] } });
        let rust = json!({ "song": { "channels": [{ "name": "Kick" }, { "name": "Hat" }, { "name": "Pad" }] } });
        let d = differences(&cs, &rust);
        assert_eq!(
            d.iter().map(|d| d.path.as_str()).collect::<Vec<_>>(),
            ["song.channels[1].name", "song.channels.len"]
        );
        assert_eq!(d[0].kind(), "song.channels[].name");
        assert_eq!((d[1].cs.as_str(), d[1].rust.as_str()), ("2", "3"));
    }

    #[test]
    fn a_map_of_samples_keeps_their_names_out_of_the_path() {
        let cs = json!({ "info": { "unplayed": { "C:\\Kick.wav": "NoNotes" } } });
        let rust = json!({ "info": { "unplayed": {} } });
        let d = differences(&cs, &rust);
        assert_eq!(d[0].kind(), "info.unplayed{}");
        assert_eq!(d[0].cs, "C:\\Kick.wav: \"NoNotes\"");
        assert_eq!(d[0].rust, "(none)");
    }
}
