//! Copies of a sound's edits, for dragging into a DAW or keeping (the 1.1 plan's light sample edits): a part picked on
//! its card's waveform ("Loop 128 (part 1.406-1.875 s, 1a2b3c4d).wav"), and a sound as the Edit strip's edits leave
//! it ("Loop 128 (edited 5e6f7a8b, 1a2b3c4d).wav").
//!
//! The original is never changed or moved (rule 2). Copies go in `Edited sounds` in the export folder, beside
//! `Trimmed sounds`: they're yours, as DAW projects go on using them, so the app never tidies them away (only a copy
//! whose making was cut short, [`is_half_made`]). A copy's name says which part of which sound it is, or which edits
//! of it (`mo_sounds::trimmed::part_name`, `edit_name`), so the same part or the same edits again reuse the copy, an
//! original changed since gets it made again, and a project that uses it still counts as using the original.
//!
//! Slices for a drum machine's pads (the Edit strip's Slices) go in a folder of their own there, one per slicing
//! ("Loop 128 slices (5e6f7a8b)", `mo_sounds::trimmed::slices_folder`), numbered in their order.
//!
//! Also where a sound's hits are, for snapping a picked part's edges to them ([`hits_of`], [`hits_in`]), and where
//! the cuts for slices go ([`cuts_at_hits`], [`cuts_at_beats`]).

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use mo_audio::edit;

/// The export folder's folder for edited copies.
pub const FOLDER: &str = "Edited sounds";

/// The longest part that is copied: five minutes of 48 kHz stereo is 115 MB as it's read. A longer part is dragged as
/// the whole file.
pub const MOST_SECONDS: f64 = 5.0 * 60.0;

/// The longest sound whose hits are found ([`hits_of`]): two minutes is 23 MB of 48 kHz mono as it's read. On a longer
/// sound's waveform the hits are too close together on the card to snap to.
pub const HITS_MOST_SECONDS: f64 = 2.0 * 60.0;

/// The shortest part worth a copy (seconds): a press that barely moved isn't a part.
pub const LEAST_PART_SECONDS: f64 = 0.010;

/// The longest sound the Edit strip edits, or the longest part of a longer one: two minutes of 48 kHz stereo is 46 MB
/// as it's read, and the strip holds that and the edit made of it.
pub const EDIT_MOST_SECONDS: f64 = 2.0 * 60.0;

/// The most slices a sound is cut into: four pages of a Drum Machine's pads, from C1 up.
pub const MOST_SLICES: usize = 64;

/// The shortest slice (seconds): a cut nearer than this to the one before it, or to the sound's end, is left out.
pub const LEAST_SLICE_SECONDS: f64 = 0.030;

/// What the Edit strip edits: the sound at `path`, whole; or, for one longer than [`EDIT_MOST_SECONDS`], the part
/// picked on its card, `span` seconds into it.
#[derive(Clone, Debug, PartialEq)]
pub struct Source {
    pub path: PathBuf,
    pub span: Option<(f64, f64)>,
}

/// What the Edit strip read of a sound ([`read_to_edit`]).
#[derive(Clone, Debug)]
pub struct ToEdit {
    pub audio: mo_audio::AudioData,
    /// When the file was last changed as it was read: a copy is made only of the file as it still is.
    pub changed: Option<SystemTime>,
}

/// Why [`read_to_edit`] read nothing.
#[derive(Clone, Debug, PartialEq)]
pub enum Unread {
    /// The sound is longer than [`EDIT_MOST_SECONDS`]: a part of it picked on its card is edited instead.
    TooLong,
    /// It couldn't be read, or there's no sound in it: why.
    Failed(String),
}

impl std::fmt::Display for Unread {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Unread::TooLong => write!(
                f,
                "It's longer than {} minutes: pick a part of it on its card to edit that part.",
                EDIT_MOST_SECONDS / 60.0
            ),
            Unread::Failed(why) => f.write_str(why),
        }
    }
}

/// Reads what the Edit strip edits ([`Source`]), all of it, with the file's date; or why it can't: it's longer than
/// [`EDIT_MOST_SECONDS`], the file can't be read, or there's no sound in it. Never on the window's thread.
pub fn read_to_edit(source: &Source) -> Result<ToEdit, Unread> {
    let path = &source.path;
    let failed = |e: String| Unread::Failed(format!("Couldn't read {}: {e}", path.display()));
    // Before the read: a change while it reads leaves the date older than the file's, so the copy waits for a read of
    // the file as it is.
    let changed = std::fs::metadata(path).map_err(|e| failed(e.to_string()))?.modified().ok();
    let audio = match source.span {
        Some((from, to)) => {
            if to - from > EDIT_MOST_SECONDS + 0.001 {
                return Err(Unread::Failed(format!(
                    "The part picked is longer than {} minutes, too long to edit.",
                    EDIT_MOST_SECONDS / 60.0
                )));
            }
            read_part(path, from, to).map_err(failed)?
        }
        None => {
            let audio = mo_audio::read(path, EDIT_MOST_SECONDS + 1.0).map_err(|e| failed(e.to_string()))?;
            if !edit::is_whole(&audio) {
                return Err(Unread::TooLong);
            }
            audio
        }
    };
    if audio.sample_rate == 0 || audio.is_empty() {
        return Err(Unread::Failed("There's no sound in it to edit.".to_owned()));
    }
    Ok(ToEdit { audio, changed })
}

/// The copy of the sound `source` as `edits` leave it, made now or before, in [`FOLDER`] in the export folder; or why
/// there's none. `audio` is the sound as [`read_to_edit`] read it, when the file was last changed `read_changed`.
/// Never on the window's thread.
pub fn edit_to_keep(
    source: &Source,
    audio: &mo_audio::AudioData,
    read_changed: Option<SystemTime>,
    edits: &[edit::Edit],
) -> Result<PathBuf, String> {
    let folder = crate::trim_copy::exports_now()?.join(FOLDER);
    edit_in(source, audio, read_changed, edits, &folder)
}

/// [`edit_to_keep`]'s work, with copies kept in `folder`.
pub fn edit_in(
    source: &Source,
    audio: &mo_audio::AudioData,
    read_changed: Option<SystemTime>,
    edits: &[edit::Edit],
    folder: &Path,
) -> Result<PathBuf, String> {
    let original = &source.path;
    if edits.is_empty() {
        return Err("There are no edits to save yet.".to_owned());
    }
    let changed = unchanged_since(original, read_changed)?;
    // The part read (to the millisecond) and every edit, written out in full: the same edits of the same part are the
    // same copy.
    let ms = |s: f64| if s.is_finite() { (s.max(0.0) * 1000.0).round() as u64 } else { 0 };
    let what = format!("{:?} {edits:?}", source.span.map(|(from, to)| (ms(from), ms(to))));
    let copy = folder.join(mo_sounds::trimmed::edit_name(&original.to_string_lossy(), &what));
    if fresh(&copy, changed) {
        return Ok(copy);
    }
    let _making = MAKING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if fresh(&copy, changed) {
        return Ok(copy);
    }
    let edited = edit::apply(audio, edits);
    if edited.is_empty() {
        return Err("The edits leave no sound to save.".to_owned());
    }
    write_new(&edited, &copy, changed)?;
    Ok(copy)
}

/// Where the slices of a sound `seconds` long start, cut at its hits (`hits`: each one's place in seconds and its
/// rise in dB, in order) that rise by `rise_db` or more: at most [`MOST_SLICES`] of them, the biggest rises when there
/// are more, none nearer than [`LEAST_SLICE_SECONDS`] to the one before or to the end. What comes before the first hit
/// is left out. With no hit that big, the whole sound is the one slice.
pub fn cuts_at_hits(hits: &[(f64, f32)], rise_db: f32, seconds: f64) -> Vec<f64> {
    let mut kept: Vec<(f64, f32)> = hits
        .iter()
        .copied()
        .filter(|(at, rise)| *rise >= rise_db && at.is_finite() && *at >= 0.0 && *at <= seconds - LEAST_SLICE_SECONDS)
        .collect();
    kept.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut cuts: Vec<(f64, f32)> = Vec::new();
    for hit in kept {
        match cuts.last_mut() {
            // Too near the one before: the bigger of the two stays.
            Some(last) if hit.0 - last.0 < LEAST_SLICE_SECONDS => {
                if hit.1 > last.1 {
                    *last = hit;
                }
            }
            _ => cuts.push(hit),
        }
    }
    if cuts.len() > MOST_SLICES {
        let mut by_rise = cuts.clone();
        by_rise.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.total_cmp(&b.0)));
        let least = by_rise[MOST_SLICES - 1];
        let mut room = MOST_SLICES;
        cuts.retain(|c| {
            let keep = room > 0 && (c.1 > least.1 || (c.1 == least.1 && c.0 <= least.0));
            room -= usize::from(keep);
            keep
        });
    }
    if cuts.is_empty() && seconds > 0.0 {
        return vec![0.0];
    }
    cuts.into_iter().map(|(at, _)| at).collect()
}

/// Where the slices of a sound `seconds` long start, cut every `beats` beats at `bpm` from its start: at most
/// [`MOST_SLICES`] (the last one then runs on to the end), none shorter than [`LEAST_SLICE_SECONDS`].
pub fn cuts_at_beats(seconds: f64, bpm: f64, beats: f64) -> Vec<f64> {
    let each = 60.0 / bpm * beats;
    if !(each.is_finite() && each >= LEAST_SLICE_SECONDS && seconds > 0.0) {
        return if seconds > 0.0 { vec![0.0] } else { Vec::new() };
    }
    (0..MOST_SLICES)
        .map(|i| i as f64 * each)
        .take_while(|at| *at == 0.0 || *at <= seconds - LEAST_SLICE_SECONDS)
        .collect()
}

/// The frames of each slice of a sound `frames` long cut at `cuts` (frames, in order): from each cut to the next
/// (the last to the end); or, for `to_end`, every one on to the sound's end.
pub fn slice_spans(cuts: &[usize], frames: usize, to_end: bool) -> Vec<edit::Span> {
    cuts.iter()
        .enumerate()
        .map(|(i, &from)| {
            let till = if to_end { frames } else { cuts.get(i + 1).copied().unwrap_or(frames) };
            edit::Span::new(from.min(frames), till.min(frames))
        })
        .filter(|s| !s.is_empty())
        .collect()
}

/// The slices `spans` of the sound `source` as the Edit strip made it (`audio`: what was read, when its file was last
/// changed `read_changed`, with its edits made; `what` tells those edits from others), each a file of its own, made
/// now or before, in a folder for this slicing in [`FOLDER`] in the export folder: their paths, in order; or why
/// there are none. Never on the window's thread.
pub fn slices_to_keep(
    source: &Source,
    audio: &mo_audio::AudioData,
    read_changed: Option<SystemTime>,
    what: &str,
    spans: &[edit::Span],
) -> Result<Vec<PathBuf>, String> {
    let folder = crate::trim_copy::exports_now()?.join(FOLDER);
    slices_in(source, audio, read_changed, what, spans, &folder)
}

/// [`slices_to_keep`]'s work, with the slicings' folders kept in `folder`.
pub fn slices_in(
    source: &Source,
    audio: &mo_audio::AudioData,
    read_changed: Option<SystemTime>,
    what: &str,
    spans: &[edit::Span],
    folder: &Path,
) -> Result<Vec<PathBuf>, String> {
    let original = &source.path;
    if spans.is_empty() {
        return Err("There are no slices to send yet.".to_owned());
    }
    if spans.len() > MOST_SLICES {
        return Err(format!("That's {} slices: {MOST_SLICES} is the most.", spans.len()));
    }
    let changed = unchanged_since(original, read_changed)?;
    let slicing = slicing_text(source, what, spans);
    let path = original.to_string_lossy();
    let into = folder.join(mo_sounds::trimmed::slices_folder(&path, &slicing));
    let mut made = Vec::with_capacity(spans.len());
    for (i, &span) in spans.iter().enumerate() {
        let copy = into.join(mo_sounds::trimmed::slice_name(&path, &slicing, i + 1));
        if !fresh(&copy, changed) {
            let _making = MAKING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if !fresh(&copy, changed) {
                let slice = edit::part(audio, span);
                if slice.is_empty() {
                    return Err(format!("Slice {} has no sound in it.", i + 1));
                }
                write_new(&slice, &copy, changed)?;
            }
        }
        made.push(copy);
    }
    Ok(made)
}

/// The name of the folder the slices `spans` of `source` go in ([`slices_to_keep`]; `what` tells its edits from
/// others): "Loop 128 slices (5e6f7a8b)".
pub fn slices_folder_name(source: &Source, what: &str, spans: &[edit::Span]) -> String {
    mo_sounds::trimmed::slices_folder(&source.path.to_string_lossy(), &slicing_text(source, what, spans))
}

/// What tells one slicing of a sound from another: the part read (to the millisecond), its edits and every slice's
/// frames, written out in full. The same slicing again is the same folder, and another never changes the slices a
/// project already uses.
fn slicing_text(source: &Source, what: &str, spans: &[edit::Span]) -> String {
    let ms = |s: f64| if s.is_finite() { (s.max(0.0) * 1000.0).round() as u64 } else { 0 };
    format!("{:?} {what} {spans:?}", source.span.map(|(from, to)| (ms(from), ms(to))))
}

/// When the sound at `original` was last changed, if that's still when it was read (`read_changed`); or that it has
/// changed since: a copy of the old sound would carry the new one's date, and be taken for it from then on.
fn unchanged_since(original: &Path, read_changed: Option<SystemTime>) -> Result<Option<SystemTime>, String> {
    let meta = std::fs::metadata(original).map_err(|e| format!("Couldn't look at {}: {e}", original.display()))?;
    let changed = meta.modified().ok();
    if changed != read_changed {
        return Err(format!(
            "{} has changed since the Edit strip opened it: open it again to edit it as it is now.",
            original.file_name().map_or_else(|| original.display().to_string(), |n| n.to_string_lossy().into_owned())
        ));
    }
    Ok(changed)
}

/// Whether the copy at `copy` was made before, and the original (last changed `changed`) hasn't changed since: a copy
/// carries its original's date.
fn fresh(copy: &Path, changed: Option<SystemTime>) -> bool {
    std::fs::metadata(copy).is_ok_and(|c| c.is_file() && c.modified().ok().zip(changed).is_none_or(|(c, o)| c >= o))
}

/// The copy of the part of the sound at `original` from `from` to `to` seconds in, made now or before, in [`FOLDER`]
/// in the export folder set in Settings (looked up as trimmed copies look it up, at most every half minute:
/// `crate::trim_copy::exports_now`); or why there's none. Reads the file: never on the window's thread.
pub fn part_to_drag(original: &Path, from: f64, to: f64) -> Result<PathBuf, String> {
    let folder = crate::trim_copy::exports_now()?.join(FOLDER);
    part_in(original, from, to, &folder)
}

/// Copies are made one at a time: two drags of the same part (one let go of while its copy was still being made, and
/// the next) would otherwise write the same file under the same name at once.
static MAKING: Mutex<()> = Mutex::new(());

/// [`part_to_drag`]'s work, with copies kept in `folder`. Only the part is read ([`mo_audio::read_span`]), however
/// long the sound.
pub fn part_in(original: &Path, from: f64, to: f64, folder: &Path) -> Result<PathBuf, String> {
    let (from, to) = (from.min(to).max(0.0), from.max(to));
    if !from.is_finite() || !to.is_finite() || to - from < LEAST_PART_SECONDS {
        return Err("The part picked is too short to drag.".to_owned());
    }
    if to - from > MOST_SECONDS {
        return Err(format!("The part picked is longer than {} minutes, too long to copy.", MOST_SECONDS / 60.0));
    }
    let meta = std::fs::metadata(original).map_err(|e| format!("Couldn't look at {}: {e}", original.display()))?;
    let changed = meta.modified().ok();
    let copy = folder.join(mo_sounds::trimmed::part_name(&original.to_string_lossy(), from, to));
    if fresh(&copy, changed) {
        return Ok(copy);
    }
    let _making = MAKING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    // Another drag may have made it while this one waited.
    if fresh(&copy, changed) {
        return Ok(copy);
    }
    let part = read_part(original, from, to).map_err(|e| format!("Couldn't read {}: {e}", original.display()))?;
    write_new(&part, &copy, changed)?;
    Ok(copy)
}

/// The part of the sound at `path` from `from` to `to` seconds in, as it's dragged, saved or played: faded in where
/// it was cut from what came before, and out where it was cut from what comes after ([`edit::ease_cuts`]). Only the
/// part is read. Or why there's none (without the file's name, which the caller adds): the file can't be read, or the
/// part starts past its end.
pub fn read_part(path: &Path, from: f64, to: f64) -> Result<mo_audio::AudioData, String> {
    let mut part = mo_audio::read_span(path, from, to - from).map_err(|e| e.to_string())?;
    if part.sample_rate == 0 || part.is_empty() {
        return Err("the part picked is past the end of the file".to_owned());
    }
    let asked = mo_audio::frame_at(to - from, part.sample_rate);
    let cut_start = mo_audio::frame_at(from, part.sample_rate) > 0;
    // All of it read: the sound went on past it (or ended just there, where a short fade does no harm).
    let cut_end = part.len() as u64 >= asked;
    edit::ease_cuts(&mut part, cut_start, cut_end);
    Ok(part)
}

/// Writes `audio` to `copy` (its folder made if need be), dated `changed` (the original's date): 24-bit, or 32-bit
/// float when it goes over full scale (so nothing is clipped). Written under a name of this program's own and then
/// renamed, so two programs making the same copy never write into one file, and a half-made copy never takes the
/// copy's name.
fn write_new(audio: &mo_audio::AudioData, copy: &Path, changed: Option<SystemTime>) -> Result<(), String> {
    if let Some(folder) = copy.parent() {
        std::fs::create_dir_all(folder).map_err(|e| format!("Couldn't make {}: {e}", folder.display()))?;
    }
    let bits = if edit::peak(audio) > 1.0 { 32 } else { 24 };
    let mine = copy.with_extension(format!("{}.wav", std::process::id()));
    let written = mo_audio::write_wav(&mine, &audio.channels, audio.sample_rate, bits)
        .map_err(|e| e.to_string())
        .and_then(|()| check(&mine, audio))
        .and_then(|()| stamp(&mine, changed).map_err(|e| e.to_string()))
        .and_then(|()| std::fs::rename(&mine, copy).map_err(|e| e.to_string()));
    written.map_err(|e| {
        let _ = std::fs::remove_file(&mine);
        format!("Couldn't write {}: {e}", copy.display())
    })
}

/// Reads `written` back and checks it's `audio`: as many channels and frames, at the same rate.
fn check(written: &Path, audio: &mo_audio::AudioData) -> Result<(), String> {
    let back = mo_audio::read(written, MOST_SECONDS).map_err(|e| format!("it didn't read back ({e})"))?;
    let same = back.sample_rate == audio.sample_rate
        && back.channels.len() == audio.channels.len()
        && back.len() == audio.len();
    if same { Ok(()) } else { Err("it didn't read back as written".to_owned()) }
}

/// Dates the file at `path` `when`, when there's a date to give.
fn stamp(path: &Path, when: Option<SystemTime>) -> std::io::Result<()> {
    match when {
        Some(when) => std::fs::File::options().write(true).open(path)?.set_modified(when),
        None => Ok(()),
    }
}

/// Whether `name` is a copy whose making was cut short, in [`FOLDER`]: written under this program's own name first
/// (`<copy>.<pid>.wav`, and `write_wav`'s `<copy>.<pid>.wav.part` beside it) and renamed after (for the start-up
/// sweep). Never a copy itself.
pub fn is_half_made(name: &str) -> bool {
    let Some(rest) = name.strip_suffix(".wav.part").or_else(|| name.strip_suffix(".wav")) else { return false };
    let Some((copy, pid)) = rest.rsplit_once('.') else { return false };
    !pid.is_empty()
        && pid.bytes().all(|b| b.is_ascii_digit())
        && mo_sounds::trimmed::mark_in(&format!("{copy}.wav")).is_some()
}

/// Where a sound's hits are: each hit's place in seconds and how big its rise was (dB), and how long the sound is.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Hits {
    pub at: Vec<(f64, f32)>,
    pub seconds: f64,
}

/// The hits of the sound at `path` ([`mo_audio::hits`]), reading the whole file, a part at a time into one channel:
/// never on the window's thread. None for a sound longer than [`HITS_MOST_SECONDS`].
pub fn hits_of(path: &Path) -> Result<Hits, String> {
    let mut mono: Vec<f32> = Vec::new();
    let mut rate = 0u32;
    let mut too_long = false;
    let seconds = mo_audio::read_parts(path, 1.0, &mut |r, part| {
        rate = r;
        let frames = part.iter().map(Vec::len).min().unwrap_or(0);
        let share = 1.0 / part.len().max(1) as f32;
        mono.extend((0..frames).map(|i| part.iter().map(|c| c[i]).sum::<f32>() * share));
        too_long = mono.len() as f64 > HITS_MOST_SECONDS * f64::from(r);
        !too_long
    })
    .map_err(|e| format!("Couldn't read {}: {e}", path.display()))?;
    if too_long || rate == 0 {
        return Ok(Hits { at: Vec::new(), seconds });
    }
    Ok(Hits { at: hits_of_mono(&mono, rate), seconds: mono.len() as f64 / f64::from(rate) })
}

/// The hits of a sound read whole (an edit in the Edit strip), as [`hits_of`] finds them: each one's place in seconds
/// and its rise (dB). None for one longer than [`HITS_MOST_SECONDS`].
pub fn hits_in(audio: &mo_audio::AudioData) -> Vec<(f64, f32)> {
    let frames = audio.channels.iter().map(Vec::len).min().unwrap_or(0);
    if audio.sample_rate == 0 || frames as f64 > HITS_MOST_SECONDS * f64::from(audio.sample_rate) {
        return Vec::new();
    }
    let share = 1.0 / audio.channels.len().max(1) as f32;
    let mono: Vec<f32> = (0..frames).map(|i| audio.channels.iter().map(|c| c[i]).sum::<f32>() * share).collect();
    hits_of_mono(&mono, audio.sample_rate)
}

fn hits_of_mono(mono: &[f32], rate: u32) -> Vec<(f64, f32)> {
    let each = f64::from(rate);
    mo_audio::hits::find_in(mono, rate).into_iter().map(|h| (h.at as f64 / each, h.rise_db)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mo-edit-copy-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 1,000 frames a second, two channels: a ramp up over one second.
    fn ramp(path: &Path) {
        let ch: Vec<f32> = (0..1_000).map(|i| i as f32 / 1_000.0).collect();
        mo_audio::write_wav(path, &[ch.clone(), ch], 1_000, 24).unwrap();
    }

    #[test]
    fn a_part_is_cut_out_once_and_reused() {
        let dir = folder("part");
        let original = dir.join("Loop 128.wav");
        ramp(&original);
        let copies = dir.join("Edited sounds");
        let copy = part_in(&original, 0.5, 0.25, &copies).unwrap();
        assert_eq!(copy.parent(), Some(copies.as_path()), "its folder made");
        let name = copy.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(name, mo_sounds::trimmed::part_name(&original.to_string_lossy(), 0.25, 0.5));
        let part = mo_audio::read(&copy, 60.0).unwrap();
        assert_eq!((part.channels.len(), part.len()), (2, 250));
        // In the middle, as it was; faded where it was cut.
        assert!((part.channels[0][125] - 0.375).abs() < 1e-4, "{}", part.channels[0][125]);
        assert_eq!(part.channels[0][0], 0.0);
        // Dated as its original, so it's fresh: dragged again, it's the same copy, not made again.
        std::fs::write(&copy, b"kept").unwrap();
        stamp(&copy, std::fs::metadata(&original).unwrap().modified().ok()).unwrap();
        assert_eq!(part_in(&original, 0.25, 0.5, &copies).unwrap(), copy);
        assert_eq!(std::fs::read(&copy).unwrap(), b"kept");
        // Nothing half-made is left beside it.
        let names: Vec<String> = std::fs::read_dir(&copies)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec![name]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_part_too_short_or_past_the_end_or_of_no_file_is_refused() {
        let dir = folder("refused");
        let original = dir.join("Kick.wav");
        ramp(&original);
        let copies = dir.join("Edited sounds");
        assert!(part_in(&original, 0.5, 0.505, &copies).is_err());
        assert!(part_in(&original, 2.0, 3.0, &copies).unwrap_err().contains("past the end"));
        assert!(part_in(&original, f64::NAN, 0.5, &copies).is_err());
        assert!(part_in(&dir.join("gone.wav"), 0.1, 0.5, &copies).is_err());
        assert!(!copies.exists(), "nothing made");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_part_is_faded_only_where_it_was_cut() {
        let dir = folder("edges");
        let original = dir.join("Pad.wav");
        mo_audio::write_wav(&original, &[vec![0.5; 1_000]], 1_000, 24).unwrap();
        // From the start: not faded in, but faded out where it was cut from the rest.
        let head = read_part(&original, 0.0, 0.5).unwrap();
        assert_eq!(head.len(), 500);
        assert!((head.channels[0][0] - 0.5).abs() < 1e-6);
        assert_eq!(head.channels[0][499], 0.0);
        // To the end (asked past it): faded in, not out; only as long as there is.
        let tail = read_part(&original, 0.5, 2.0).unwrap();
        assert_eq!(tail.len(), 500);
        assert_eq!(tail.channels[0][0], 0.0);
        assert!((tail.channels[0][499] - 0.5).abs() < 1e-6);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_part_that_goes_over_full_scale_is_kept_as_floats() {
        let dir = folder("loud");
        let original = dir.join("Loud.wav");
        mo_audio::write_wav(&original, &[vec![1.5; 1_000]], 1_000, 32).unwrap();
        let copy = part_in(&original, 0.0, 0.5, &dir.join("Edited sounds")).unwrap();
        let part = mo_audio::read(&copy, 60.0).unwrap();
        assert!((part.channels[0][100] - 1.5).abs() < 1e-6, "not clipped");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn half_made_copies_are_told_from_copies() {
        let copy = mo_sounds::trimmed::part_name("D:\\Samples\\Loop.wav", 1.0, 2.0);
        let stem = copy.strip_suffix(".wav").unwrap();
        assert!(is_half_made(&format!("{stem}.4242.wav")));
        assert!(is_half_made(&format!("{stem}.4242.wav.part")));
        assert!(!is_half_made(&copy), "never a copy");
        assert!(!is_half_made("Loop.4242.wav"), "never a file of the user's");
    }

    #[test]
    fn hits_are_found_in_seconds() {
        let dir = folder("hits");
        let original = dir.join("Two hits.wav");
        let mut ch = vec![0.0f32; 48_000];
        for at in [12_000, 36_000] {
            for (i, s) in ch[at..at + 4_800].iter_mut().enumerate() {
                *s = (-(i as f32) / 800.0).exp() * if i % 2 == 0 { 0.8 } else { -0.8 };
            }
        }
        mo_audio::write_wav(&original, &[ch], 48_000, 24).unwrap();
        let hits = hits_of(&original).unwrap();
        assert!((hits.seconds - 1.0).abs() < 1e-9);
        // The same from the sound read whole, as the Edit strip finds them.
        assert_eq!(hits_in(&mo_audio::read(&original, 60.0).unwrap()), hits.at);
        let at: Vec<f64> = hits.at.iter().map(|h| h.0).collect();
        assert_eq!(at.len(), 2, "{at:?}");
        assert!((at[0] - 0.25).abs() < 0.003 && (at[1] - 0.75).abs() < 0.003, "{at:?}");
        // Longer than two minutes: none, as they'd be too close together on the card to snap to.
        let long = dir.join("Long.wav");
        mo_audio::write_wav(&long, &[vec![0.0; 121_000]], 1_000, 16).unwrap();
        let hits = hits_of(&long).unwrap();
        assert!(hits.at.is_empty());
        assert!(hits.seconds > HITS_MOST_SECONDS, "{}", hits.seconds);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_edit_is_saved_once_and_reused() {
        use mo_audio::edit::{Edit, Span};
        let dir = folder("edit");
        let original = dir.join("Loop 128.wav");
        ramp(&original);
        let source = Source { path: original.clone(), span: None };
        let ToEdit { audio, changed } = read_to_edit(&source).unwrap();
        assert_eq!(audio.len(), 1_000);
        let copies = dir.join("Edited sounds");
        let edits = [Edit::Reverse(None)];
        let copy = edit_in(&source, &audio, changed, &edits, &copies).unwrap();
        assert_eq!(copy.parent(), Some(copies.as_path()), "its folder made");
        let name = copy.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("Loop 128 (edited "), "{name}");
        let mark = mo_sounds::trimmed::mark(&original.to_string_lossy());
        assert_eq!(mo_sounds::trimmed::mark_in(&name), Some(mark.as_str()));
        let back = mo_audio::read(&copy, 60.0).unwrap();
        assert_eq!((back.channels.len(), back.len()), (2, 1_000));
        assert!((back.channels[0][0] - 0.999).abs() < 1e-4, "played backwards: {}", back.channels[0][0]);
        // The same edits again: the copy made before, as it is (not written again).
        std::fs::write(&copy, b"kept").unwrap();
        assert_eq!(edit_in(&source, &audio, changed, &edits, &copies).unwrap(), copy);
        assert_eq!(std::fs::read(&copy).unwrap(), b"kept");
        // Other edits, or the same edits of a part: other copies.
        let louder = edit_in(&source, &audio, changed, &[Edit::Normalise { peak_db: 0.0 }], &copies).unwrap();
        assert_ne!(louder, copy);
        let part = Source { path: original.clone(), span: Some((0.25, 0.5)) };
        let part_audio = read_to_edit(&part).unwrap().audio;
        assert_eq!(part_audio.len(), 250);
        let part_copy = edit_in(&part, &part_audio, changed, &edits, &copies).unwrap();
        assert!(part_copy != copy && part_copy != louder);
        assert_eq!(mo_audio::read(&part_copy, 60.0).unwrap().len(), 250);
        // A trim's copy is as long as the trim.
        let trimmed = edit_in(&source, &audio, changed, &[Edit::Trim(Span::new(100, 300))], &copies).unwrap();
        assert_eq!(mo_audio::read(&trimmed, 60.0).unwrap().len(), 200);
        // No edits: nothing to save.
        assert!(edit_in(&source, &audio, changed, &[], &copies).is_err());
        // The file changed after it was read: no copy of the old sound under the new one's date.
        let earlier = changed.and_then(|c| c.checked_sub(std::time::Duration::from_secs(60)));
        let stale = edit_in(&source, &audio, earlier, &[Edit::Reverse(Some(Span::new(0, 10)))], &copies);
        assert!(stale.as_ref().is_err_and(|e| e.contains("has changed")), "{stale:?}");
        // Nothing half made is left.
        let left: Vec<String> = std::fs::read_dir(&copies)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| is_half_made(n))
            .collect();
        assert!(left.is_empty(), "{left:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cuts_go_at_the_hits_big_enough_and_never_too_close() {
        let hits = [(0.0, 40.0), (0.5, 8.0), (1.0, 20.0), (1.01, 30.0), (1.5, 12.0), (1.99, 50.0)];
        // Every hit of 12 dB or more: the nearer of two too close gives way to the bigger, and one too near the end
        // is left out.
        assert_eq!(cuts_at_hits(&hits, 12.0, 2.0), vec![0.0, 1.01, 1.5]);
        assert_eq!(cuts_at_hits(&hits, 6.0, 2.0), vec![0.0, 0.5, 1.01, 1.5]);
        // None that big: the whole sound is the one slice.
        assert_eq!(cuts_at_hits(&hits, 60.0, 2.0), vec![0.0]);
        assert!(cuts_at_hits(&hits, 6.0, 0.0).is_empty());
        // More than the most: the biggest rises stay, in their order.
        let many: Vec<(f64, f32)> =
            (0..100).map(|i| (f64::from(i) * 0.1, if i % 2 == 0 { 30.0 } else { 10.0 })).collect();
        let cuts = cuts_at_hits(&many, 6.0, 20.0);
        assert_eq!(cuts.len(), MOST_SLICES);
        assert!(cuts.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(cuts.iter().filter(|c| ((*c / 0.1).round() as i64) % 2 == 0).count(), 50, "every big one");
    }

    #[test]
    fn cuts_go_on_the_beats() {
        // 120 BPM: a beat is half a second.
        assert_eq!(cuts_at_beats(2.0, 120.0, 1.0), vec![0.0, 0.5, 1.0, 1.5]);
        assert_eq!(cuts_at_beats(2.0, 120.0, 4.0), vec![0.0]);
        assert_eq!(cuts_at_beats(2.01, 120.0, 0.5).len(), 4 * 2, "no sliver of a slice at the end");
        assert_eq!(cuts_at_beats(600.0, 120.0, 0.25).len(), MOST_SLICES);
        // No tempo to speak of: one slice.
        assert_eq!(cuts_at_beats(2.0, 0.0, 1.0), vec![0.0]);
        assert_eq!(cuts_at_beats(2.0, f64::NAN, 1.0), vec![0.0]);
        assert!(cuts_at_beats(0.0, 120.0, 1.0).is_empty());
    }

    #[test]
    fn slices_run_to_the_next_cut_or_to_the_end() {
        use mo_audio::edit::Span;
        assert_eq!(
            slice_spans(&[0, 250, 600], 1_000, false),
            [Span::new(0, 250), Span::new(250, 600), Span::new(600, 1_000)]
        );
        assert_eq!(slice_spans(&[100, 600], 1_000, true), [Span::new(100, 1_000), Span::new(600, 1_000)]);
        assert!(slice_spans(&[1_000], 1_000, false).is_empty(), "a cut at the end is no slice");
    }

    #[test]
    fn slices_are_saved_once_in_a_folder_of_their_slicing() {
        use mo_audio::edit::Span;
        let dir = folder("slices");
        let original = dir.join("Loop 128.wav");
        ramp(&original);
        let source = Source { path: original.clone(), span: None };
        let ToEdit { audio, changed } = read_to_edit(&source).unwrap();
        let copies = dir.join("Edited sounds");
        let spans = slice_spans(&[0, 250, 600], audio.len(), false);
        let made = slices_in(&source, &audio, changed, "[]", &spans, &copies).unwrap();
        assert_eq!(made.len(), 3);
        let into = made[0].parent().unwrap();
        assert_eq!(into.parent(), Some(copies.as_path()));
        assert!(mo_sounds::trimmed::is_slices_folder(&into.file_name().unwrap().to_string_lossy()));
        assert_eq!(into.file_name().unwrap().to_string_lossy(), slices_folder_name(&source, "[]", &spans));
        let names: Vec<String> = made.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert!(
            names[0].starts_with("Loop 128 01 (slice ") && names[2].starts_with("Loop 128 03 (slice "),
            "{names:?}"
        );
        let mark = mo_sounds::trimmed::mark(&original.to_string_lossy());
        assert!(names.iter().all(|n| mo_sounds::trimmed::mark_in(n) == Some(mark.as_str())));
        let lengths: Vec<usize> = made.iter().map(|p| mo_audio::read(p, 60.0).unwrap().len()).collect();
        assert_eq!(lengths, [250, 350, 400]);
        // The same slicing again: the slices made before, as they are.
        std::fs::write(&made[1], b"kept").unwrap();
        assert_eq!(slices_in(&source, &audio, changed, "[]", &spans, &copies).unwrap(), made);
        assert_eq!(std::fs::read(&made[1]).unwrap(), b"kept");
        // Other cuts, or the same cuts of other edits: another folder, and the first one's slices stay as they were.
        let other =
            slices_in(&source, &audio, changed, "[]", &[Span::new(0, 500), Span::new(500, 1_000)], &copies).unwrap();
        assert_ne!(other[0].parent(), made[0].parent());
        let edited = slices_in(&source, &audio, changed, "[Reverse(None)]", &spans, &copies).unwrap();
        assert_ne!(edited[0].parent(), made[0].parent());
        assert_eq!(std::fs::read(&made[1]).unwrap(), b"kept");
        // None, too many, or a file changed since it was read: no slices.
        assert!(slices_in(&source, &audio, changed, "[]", &[], &copies).is_err());
        let many = vec![Span::new(0, 10); MOST_SLICES + 1];
        assert!(slices_in(&source, &audio, changed, "[]", &many, &copies).is_err());
        let earlier = changed.and_then(|c| c.checked_sub(std::time::Duration::from_secs(60)));
        let stale = slices_in(&source, &audio, earlier, "[]", &[Span::new(0, 100)], &copies);
        assert!(stale.as_ref().is_err_and(|e| e.contains("has changed")), "{stale:?}");
        // Nothing half made is left.
        let left: Vec<String> = std::fs::read_dir(into)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| is_half_made(n))
            .collect();
        assert!(left.is_empty(), "{left:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_sound_too_long_to_edit_whole_is_edited_a_part_at_a_time() {
        let dir = folder("long-edit");
        let long = dir.join("Long.wav");
        mo_audio::write_wav(&long, &[vec![0.25; 125_000]], 1_000, 16).unwrap();
        let whole = read_to_edit(&Source { path: long.clone(), span: None }).err();
        assert_eq!(whole, Some(Unread::TooLong));
        assert!(Unread::TooLong.to_string().contains("longer than 2 minutes"));
        let part = read_to_edit(&Source { path: long.clone(), span: Some((10.0, 20.0)) }).unwrap().audio;
        assert_eq!(part.len(), 10_000);
        assert!(read_to_edit(&Source { path: long, span: Some((0.0, 124.0)) }).is_err(), "a part too long");
        let empty = dir.join("Empty.wav");
        mo_audio::write_wav(&empty, &[Vec::new()], 1_000, 16).unwrap();
        let empty = read_to_edit(&Source { path: empty, span: None }).err();
        assert!(matches!(empty, Some(Unread::Failed(_))), "{empty:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
