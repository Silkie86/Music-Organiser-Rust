//! Random kits that still make sense: each pad gets a sound of the right type, optionally favouring sounds you
//! haven't used, sticking to one pack for a coherent character, matching a described vibe (with deep listening), and
//! tuning bass and 808 pads to a key. Locked pads stay put on re-rolls. Then the kit can be copied into a folder of
//! its own, ready for a sampler. A port of the C# app's `KitBuilder`, checked by the sounds twin test.
//!
//! Rolling is quick; exporting reads and writes audio: call it on the app's low-priority pool, never the window's
//! thread.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::dotnet_random::DotnetRandom;
use crate::index::{SoundIndex, dot};
use crate::usage::Usage;
use crate::{Sound, directory, types};

/// One pad in a kit: which sound types can fill it (the first fits best), and what's on it now.
#[derive(Clone, Debug, PartialEq)]
pub struct KitSlot {
    pub name: String,
    pub types: Vec<String>,
    /// Kept as it is when the kit is rolled again.
    pub locked: bool,
    /// The sound's id.
    pub sound: Option<i64>,
}

impl KitSlot {
    fn new(name: &str, types: &[&str]) -> KitSlot {
        KitSlot {
            name: name.to_owned(),
            types: types.iter().map(|t| (*t).to_owned()).collect(),
            locked: false,
            sound: None,
        }
    }
}

/// How to roll a kit.
#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    pub prefer_unused: bool,
    /// Everything from one pack (the pack of a locked pad's sound, else one picked that can fill most pads).
    pub same_pack: bool,
    /// A described vibe ("dark gritty"), as deep listening's model turned it into a vector: the caller does that,
    /// since the model lives in the app. Used only once deep listening has heard most sounds.
    pub vibe: Option<Vec<f32>>,
    /// 0 (C) to 11 (B): bass and 808 pads near this note are preferred.
    pub key_tonic: Option<i32>,
    /// The same seed rolls the same kit; 0 rolls a new one.
    pub seed: i32,
    /// 0: the best fits; 1: anything that fits.
    pub randomness: f64,
}

impl Default for Options {
    fn default() -> Self {
        Options { prefer_unused: true, same_pack: false, vibe: None, key_tonic: None, seed: 0, randomness: 0.5 }
    }
}

/// The usual ten pads of a drum kit.
pub fn default_drum_kit() -> Vec<KitSlot> {
    vec![
        KitSlot::new("Kick", &["Kick"]),
        KitSlot::new("Snare", &["Snare"]),
        KitSlot::new("Clap", &["Clap", "Snare"]),
        KitSlot::new("Closed hat", &["Closed hat", "Hi-hat"]),
        KitSlot::new("Open hat", &["Open hat", "Hi-hat", "Cymbal"]),
        KitSlot::new("Percussion", &["Percussion", "Tom"]),
        KitSlot::new("Percussion 2", &["Percussion", "Tom", "Clap"]),
        KitSlot::new("Cymbal", &["Cymbal", "Open hat"]),
        KitSlot::new("808 / Bass", &["808", "Bass"]),
        KitSlot::new("FX", &["FX", "Texture", "Vocal"]),
    ]
}

/// The pack a sample belongs to: the folder two levels above it, or its own folder (as C#'s `KitBuilder.Pack`; on
/// Windows with `/` turned into `\`, as .NET does, so one pack spelt both ways is one).
pub fn pack(s: &Sound) -> String {
    let dir = directory(&s.path);
    let parent = directory(dir);
    let p = if parent.is_empty() { dir } else { parent };
    if cfg!(windows) { p.replace('/', "\\") } else { p.to_owned() }
}

fn is_loop(s: &Sound) -> bool {
    s.analysis.as_ref().is_some_and(|a| a.is_loop)
}

/// Fills every unlocked pad. Returns the seed used, so the kit can be rolled again the same. `sounds` is the whole
/// library (hidden copies are left out here), `usage` from [`crate::usage::usage`], `index` from [`SoundIndex::read`].
pub fn roll(
    sounds: &[Sound],
    usage: &HashMap<i64, Usage>,
    index: &SoundIndex,
    slots: &mut [KitSlot],
    o: &Options,
) -> i32 {
    let seed = if o.seed != 0 { o.seed } else { crate::rediscover::fresh_seed() };
    let mut rng = DotnetRandom::new(seed);
    let clap = index.clap.as_ref().filter(|_| index.deep);
    let vibe = o.vibe.as_deref().filter(|_| clap.is_some());
    // A locked pad keeps its sound whatever has happened to it since (hidden, or gone in a rescan), as in C#.
    let is_locked = |slot: &KitSlot| slot.locked && slot.sound.is_some();
    let locked_ids: Vec<i64> = slots.iter().filter(|s| is_locked(s)).filter_map(|s| s.sound).collect();
    let locked_sound = |id: i64| sounds.iter().find(|s| s.id == id);

    // The pack everything should come from, when asked.
    let mut the_pack: Option<String> = None;
    if o.same_pack {
        the_pack = locked_ids.iter().find_map(|id| locked_sound(*id)).map(pack);
        if the_pack.is_none() {
            let visible: Vec<&Sound> = sounds.iter().filter(|s| !s.hidden).collect();
            the_pack = pick_pack(&visible, slots, &mut rng);
        }
    }

    let mut used = locked_ids;
    for slot in slots.iter_mut() {
        if is_locked(slot) {
            continue;
        }
        let candidates: Vec<(&Sound, Option<String>)> = sounds
            .iter()
            .filter(|s| {
                !s.hidden
                    && s.analysis.is_some()
                    && !is_loop(s)
                    && slot.types.contains(&s.sound_type)
                    && !used.contains(&s.id)
            })
            .map(|s| (s, the_pack.as_ref().map(|_| pack(s))))
            .collect();
        let candidates: Vec<&Sound> = match &the_pack {
            Some(p) if candidates.iter().any(|(_, cp)| cp.as_ref() == Some(p)) => {
                candidates.into_iter().filter(|(_, cp)| cp.as_ref() == Some(p)).map(|(c, _)| c).collect()
            }
            _ => candidates.into_iter().map(|(c, _)| c).collect(),
        };
        if candidates.is_empty() {
            slot.sound = None;
            continue;
        }
        let scored: Vec<(&Sound, f64)> = candidates
            .into_iter()
            .map(|c| {
                let mut score = 0.0;
                // Earlier types in the slot are the better fit (a "Clap" pad prefers claps over snares).
                let at = slot.types.iter().position(|t| *t == c.sound_type).map_or(-1, |p| p as i64);
                score -= at as f64 * 0.3;
                let used_in = crate::rediscover::used_in(usage, c.id);
                if o.prefer_unused {
                    score += if used_in == 0 { 0.4 } else { -0.1 * f64::from(used_in.min(5)) };
                }
                if c.favourite {
                    score += 0.2;
                }
                score += f64::from((c.rating - 3).max(0)) * 0.1;
                if let (Some(vibe), Some(cv)) = (vibe, clap.and_then(|i| i.get(c.id))) {
                    score += 3.0 * dot(vibe, cv);
                }
                if let (Some(key), true, Some(p)) = (
                    o.key_tonic,
                    types::TONAL.contains(&c.sound_type.as_str()),
                    c.analysis.as_ref().and_then(|a| a.pitch),
                ) {
                    let pc = ((p.round_ties_even() as i32) % 12 + 12) % 12;
                    score += if pc == key {
                        0.6
                    } else if pc == (key + 7) % 12 {
                        0.3
                    } else {
                        -0.3
                    };
                }
                (c, score)
            })
            .collect();
        let pick = sample(&scored, o.randomness, &mut rng);
        slot.sound = Some(pick.id);
        used.push(pick.id);
    }
    seed
}

/// Softmax sampling: low randomness takes the best fits, high randomness anything that fits.
fn sample<'a>(scored: &[(&'a Sound, f64)], randomness: f64, rng: &mut DotnetRandom) -> &'a Sound {
    let temp = 0.05 + 2.0 * randomness.clamp(0.0, 1.0);
    let max = scored.iter().map(|s| s.1).fold(f64::NEG_INFINITY, f64::max);
    let weights: Vec<f64> = scored.iter().map(|s| ((s.1 - max) / temp).exp()).collect();
    let mut r = rng.next_double() * weights.iter().sum::<f64>();
    for (i, w) in weights.iter().enumerate() {
        r -= w;
        if r <= 0.0 {
            return scored[i].0;
        }
    }
    scored[scored.len() - 1].0
}

/// A pack that can fill most of the kit's pads, picked at random.
fn pick_pack(sounds: &[&Sound], slots: &[KitSlot], rng: &mut DotnetRandom) -> Option<String> {
    // Grouped in the order the packs are first met, as C#'s GroupBy.
    let mut at: HashMap<String, usize> = HashMap::new();
    let mut groups: Vec<(String, Vec<&Sound>)> = Vec::new();
    for s in sounds.iter().filter(|s| !is_loop(s)) {
        let p = pack(s);
        let i = *at.entry(p.clone()).or_insert_with(|| {
            groups.push((p, Vec::new()));
            groups.len() - 1
        });
        groups[i].1.push(s);
    }
    let need = 2.max(slots.len() / 2);
    let packs: Vec<&str> = groups
        .iter()
        .filter(|(_, g)| slots.iter().filter(|sl| g.iter().any(|s| sl.types.contains(&s.sound_type))).count() >= need)
        .map(|(p, _)| p.as_str())
        .collect();
    if packs.is_empty() {
        return None;
    }
    let n = i32::try_from(packs.len()).unwrap_or(i32::MAX);
    let i = usize::try_from(rng.next_in(0, n)).unwrap_or(0);
    Some(packs[i.min(packs.len() - 1)].to_owned())
}

/// How to copy a kit out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExportOptions {
    /// 48 kHz 16-bit WAV, for the Digitakt II or Analog Rytm via Elektron Transfer.
    pub elektron_ready: bool,
    pub mono: bool,
    /// Each pad as loud as it can go without clipping.
    pub normalise: bool,
}

/// An exported kit: its folder, and the pad files written into it, in pad order.
#[derive(Clone, Debug, PartialEq)]
pub struct Exported {
    pub folder: PathBuf,
    pub pads: Vec<PathBuf>,
}

/// What stopped an export.
#[derive(Debug)]
pub enum ExportError {
    /// A pad's sample couldn't be read (a drive not plugged in, a file moved).
    Missing(PathBuf, std::io::Error),
    Io(PathBuf, std::io::Error),
    /// A copy came out a different size from its sample.
    Short(PathBuf),
    Audio(PathBuf, mo_audio::AudioError),
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExportError::Missing(p, e) => write!(f, "Couldn't read the sample {}: {e}", p.display()),
            ExportError::Io(p, e) => write!(f, "Couldn't write {}: {e}", p.display()),
            ExportError::Short(p) => write!(f, "{} didn't copy whole.", p.display()),
            ExportError::Audio(p, e) => write!(f, "Couldn't convert {}: {e}", p.display()),
        }
    }
}

impl std::error::Error for ExportError {}

/// Copies the kit into a new folder in `parent`, named after the kit, with numbered pad names ("01 Kick - Big
/// Kick.wav") and a `kit.txt` saying where each came from. A folder of that name already there (a kit exported in the
/// same minute) is left as it is, since a DAW may be using its files: the new one gets a number after the name.
/// The samples themselves are only read. Each file is written beside its final name first, then renamed.
pub fn export<'a>(
    slots: &[KitSlot],
    get: impl Fn(i64) -> Option<&'a Sound>,
    parent: &Path,
    kit_name: &str,
    o: ExportOptions,
) -> Result<Exported, ExportError> {
    let name = safe(kit_name);
    std::fs::create_dir_all(parent).map_err(|e| ExportError::Io(parent.to_owned(), e))?;
    // Made, not just looked for, so two exports at once never share a folder.
    let mut folder = parent.join(&name);
    let mut n = 2;
    loop {
        match std::fs::create_dir(&folder) {
            Ok(()) => break,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                folder = parent.join(format!("{name} ({n})"));
                n += 1;
            }
            Err(e) => return Err(ExportError::Io(folder, e)),
        }
    }
    let mut pads = Vec::new();
    let mut lines = vec![kit_name.to_owned(), String::new()];
    let mut n = 0;
    for slot in slots {
        let Some(s) = slot.sound.and_then(&get) else { continue };
        n += 1;
        let base = format!("{n:02} {} - {}", safe(&slot.name), safe(s.name()));
        let from = Path::new(&s.path);
        let target = if !o.elektron_ready && !o.mono && !o.normalise {
            let ext = from.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
            let target = folder.join(format!("{base}{ext}"));
            copy_whole(from, &target)?;
            target
        } else {
            let audio = mo_audio::read(from, 600.0).map_err(|e| ExportError::Audio(from.to_owned(), e))?;
            let rate = if o.elektron_ready { 48_000 } else { audio.sample_rate };
            // Converted straight from the decoded audio (a long sample isn't copied more than it has to be).
            let mut channels: Vec<Vec<f32>> = if o.mono {
                vec![mo_audio::dsp::resample(&audio.mono(), audio.sample_rate, rate)]
            } else if rate == audio.sample_rate {
                audio.channels
            } else {
                audio.channels.iter().map(|c| mo_audio::dsp::resample(c, audio.sample_rate, rate)).collect()
            };
            if o.normalise {
                let peak = channels.iter().flat_map(|c| c.iter()).fold(0f32, |m, x| m.max(x.abs()));
                if peak > 0.0 {
                    let g = 0.98f32 / peak;
                    for x in channels.iter_mut().flat_map(|c| c.iter_mut()) {
                        *x *= g;
                    }
                }
            }
            let target = folder.join(format!("{base}.wav"));
            // write_wav writes beside the name first and renames.
            mo_audio::write_wav(&target, &channels, rate, 16).map_err(|e| ExportError::Audio(target.clone(), e))?;
            target
        };
        pads.push(target);
        lines.push(format!("{n:02} {}: {}", slot.name, s.path));
    }
    let newline = if cfg!(windows) { "\r\n" } else { "\n" };
    let mut text = lines.join(newline);
    text.push_str(newline);
    let list = folder.join("kit.txt");
    let part = part_of(&list);
    let written = std::fs::write(&part, text).and_then(|()| std::fs::rename(&part, &list));
    if let Err(e) = written {
        let _ = std::fs::remove_file(&part);
        return Err(ExportError::Io(list, e));
    }
    Ok(Exported { folder, pads })
}

/// Copies `from` to `to`: beside it first, checked to be the whole file and on the disk, then renamed. A copy that
/// fails is removed, so no half pad is left behind.
fn copy_whole(from: &Path, to: &Path) -> Result<(), ExportError> {
    let size = std::fs::metadata(from).map_err(|e| ExportError::Missing(from.to_owned(), e))?.len();
    let part = part_of(to);
    let done = (|| {
        std::fs::copy(from, &part).map_err(|e| ExportError::Io(to.to_owned(), e))?;
        // Opened to write: Windows only flushes a file to the disk through a handle that may write to it.
        let file =
            std::fs::OpenOptions::new().write(true).open(&part).map_err(|e| ExportError::Io(to.to_owned(), e))?;
        file.sync_all().map_err(|e| ExportError::Io(to.to_owned(), e))?;
        if file.metadata().map_err(|e| ExportError::Io(to.to_owned(), e))?.len() != size {
            return Err(ExportError::Short(from.to_owned()));
        }
        drop(file);
        std::fs::rename(&part, to).map_err(|e| ExportError::Io(to.to_owned(), e))
    })();
    if done.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    done
}

/// Where a file is written before it's renamed to `path`.
fn part_of(path: &Path) -> PathBuf {
    let mut p = path.as_os_str().to_owned();
    p.push(".part");
    PathBuf::from(p)
}

/// A name that's safe as a file name on every system: characters Windows doesn't allow become "-".
fn safe(s: &str) -> String {
    s.chars()
        .map(|c| if c < ' ' || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') { '-' } else { c })
        .collect::<String>()
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SoundAnalysis;
    use crate::test_support::sound;

    /// A folder as `pack` spells it on this system (Windows turns `/` into `\`).
    fn sep(p: &str) -> String {
        if cfg!(windows) { p.replace('/', "\\") } else { p.to_owned() }
    }

    fn one_shot(id: i64, path: &str, kind: &str) -> Sound {
        let mut s = sound(id, path);
        s.analysis = Some(SoundAnalysis::default());
        s.sound_type = kind.to_owned();
        s
    }

    fn library() -> Vec<Sound> {
        let mut out = Vec::new();
        let mut id = 0;
        for pack in ["Pack A", "Pack B"] {
            for kind in ["Kick", "Snare", "Clap", "Closed hat", "808"] {
                for n in 0..3 {
                    id += 1;
                    out.push(one_shot(id, &format!("/s/{pack}/{kind}/{kind} {n}.wav"), kind));
                }
            }
        }
        out
    }

    #[test]
    fn fills_each_pad_with_the_right_type_and_no_sound_twice() {
        let sounds = library();
        let mut kit = default_drum_kit();
        let seed = roll(
            &sounds,
            &HashMap::new(),
            &SoundIndex::default(),
            &mut kit,
            &Options { seed: 4, ..Options::default() },
        );
        assert_eq!(seed, 4);
        let by_id: HashMap<i64, &Sound> = sounds.iter().map(|s| (s.id, s)).collect();
        let mut seen = Vec::new();
        for slot in &kit {
            if let Some(id) = slot.sound {
                assert!(slot.types.contains(&by_id[&id].sound_type), "{slot:?}");
                assert!(!seen.contains(&id));
                seen.push(id);
            }
        }
        // No open hats, toms, cymbals or FX in the library: those pads stay empty.
        assert_eq!(kit[4].sound, None);
        assert!(kit[0].sound.is_some() && kit[8].sound.is_some());
        // The same seed rolls the same kit.
        let mut again = default_drum_kit();
        roll(&sounds, &HashMap::new(), &SoundIndex::default(), &mut again, &Options { seed: 4, ..Options::default() });
        assert_eq!(kit, again);
    }

    #[test]
    fn locked_pads_stay_and_set_the_pack() {
        let sounds = library();
        let mut kit = default_drum_kit();
        // Lock the kick on a Pack B kick (id 16).
        kit[0].sound = Some(16);
        kit[0].locked = true;
        let o = Options { same_pack: true, seed: 9, randomness: 1.0, ..Options::default() };
        roll(&sounds, &HashMap::new(), &SoundIndex::default(), &mut kit, &o);
        assert_eq!(kit[0].sound, Some(16));
        let by_id: HashMap<i64, &Sound> = sounds.iter().map(|s| (s.id, s)).collect();
        for id in kit.iter().filter_map(|s| s.sound) {
            assert_eq!(pack(by_id[&id]), sep("/s/Pack B"), "{id}");
        }
        // A locked pad keeps its sound even once it's hidden.
        let mut sounds = sounds.clone();
        sounds[15].hidden = true;
        roll(&sounds, &HashMap::new(), &SoundIndex::default(), &mut kit, &o);
        assert_eq!(kit[0].sound, Some(16));
        for id in kit.iter().filter_map(|s| s.sound) {
            assert_eq!(pack(by_id[&id]), sep("/s/Pack B"), "{id}");
        }
    }

    #[test]
    fn low_randomness_takes_the_best_fit() {
        let mut sounds = library();
        // A starred, five-star kick, never used: the clear best.
        sounds[1].favourite = true;
        sounds[1].rating = 5;
        let usage = HashMap::from([(1, Usage { projects: 4, ..Usage::default() })]);
        for seed in 1..20 {
            let mut kit = default_drum_kit();
            roll(
                &sounds,
                &usage,
                &SoundIndex::default(),
                &mut kit,
                &Options { seed, randomness: 0.0, ..Options::default() },
            );
            assert_eq!(kit[0].sound, Some(2));
        }
    }

    #[test]
    fn key_prefers_808s_on_the_note() {
        let mut sounds = library();
        for (s, pitch) in sounds.iter_mut().filter(|s| s.sound_type == "808").zip([36.0, 43.0, 38.0, 41.0, 37.0, 40.0])
        {
            s.analysis.as_mut().unwrap().pitch = Some(pitch);
        }
        let c = sounds.iter().find(|s| s.analysis.as_ref().unwrap().pitch == Some(36.0)).unwrap().id;
        for seed in 1..20 {
            let mut kit = default_drum_kit();
            let o = Options { seed, randomness: 0.0, key_tonic: Some(0), ..Options::default() };
            roll(&sounds, &HashMap::new(), &SoundIndex::default(), &mut kit, &o);
            assert_eq!(kit[8].sound, Some(c));
        }
    }

    #[test]
    fn names_are_made_safe() {
        assert_eq!(safe(" Kick: \"Big\" / 1 "), "Kick- -Big- - 1");
        assert_eq!(pack(&sound(1, "/s/Pack/Kicks/a.wav")), sep("/s/Pack"));
        assert_eq!(pack(&sound(1, "/a.wav")), sep("/"));
        assert_eq!(pack(&sound(1, r"D:\a.wav")), r"D:\");
    }
}
