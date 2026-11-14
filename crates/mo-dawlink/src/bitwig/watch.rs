//! The watch feed (extension 3.24): the song as Bitwig shows it now, kept up to date as it changes.
//!
//! [`Watch::start`] opens a connection of its own to the link, on a thread of its own, and asks it to `watch`. The link
//! then sends what changes in Bitwig as it changes: the notes each track plays (looked at every 50 ms), and the
//! project, the transport, the tracks with their devices, the selection, the cue markers and each track's level
//! (every 100 ms). While nothing changes only an empty event comes, every 5 s, and a feed silent for [`SILENT`] counts
//! as Bitwig gone. Each event is merged into one picture, a [`Live`], which [`Watch::now`] hands out as it stands: the
//! newest always wins, and reading it never waits for Bitwig (rule 12).
//!
//! When Bitwig isn't there, closes, or runs an extension older than [`WATCH_NEEDS`], the picture says so
//! ([`Live::state`]) and the thread tries again every [`RETRY`]. Dropping the [`Watch`] ends it: its connection is
//! closed from the dropping thread, which doesn't wait for the watch's thread to finish.
//!
//! The feed only reads, so it never needs "Let the app make changes", and it changes nothing in Bitwig.

use std::net::{Shutdown, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

use serde_json::Value;

use super::link::{CONNECT_TIMEOUT, Link};

/// The extension version from which the link sends the watch feed.
pub const WATCH_NEEDS: &str = "3.24";
/// The first version of the extension whose watch feed has the song's Groove.
pub const GROOVE_NEEDS: &str = "3.27";

/// How long the watch waits before trying Bitwig again, after it wasn't there or the connection ended.
pub const RETRY: Duration = Duration::from_secs(2);

/// How long it waits when the extension is too old or refused: it can't change until Bitwig loads it again, and each
/// try is a connection in the extension's report log.
pub const RETRY_OLD: Duration = Duration::from_secs(60);

/// How long the feed may be silent before the watch takes Bitwig as gone: the link sends at least an empty event
/// every 5 seconds (`Watch.KEEP_ALIVE`), so a Bitwig that hangs with its connection open is noticed.
pub const SILENT: Duration = Duration::from_secs(15);

/// Whether the watch has Bitwig now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum State {
    /// Trying to reach Bitwig (and before the first try).
    #[default]
    Connecting,
    /// Nothing answered on the link's port: Bitwig isn't running, or the extension isn't added in it.
    NoBitwig,
    /// The link answered, but it's older than [`WATCH_NEEDS`]: what answered, in words ("the Music Organiser
    /// extension 3.23"). Installing the extension again (Settings > DAWs) brings the feed.
    TooOld(String),
    /// The link refused to watch, in its words.
    Refused(String),
    /// The feed is coming in.
    Watching,
}

/// The song as Bitwig showed it last: every section as the newest event that had it left it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Live {
    pub state: State,
    /// Which sections this Bitwig gives ("project", "tracks", "selected", "transport", "markers", "notes", "levels",
    /// and from extension 3.27 "groove"): a section missing here stays empty.
    pub sections: Vec<String>,
    /// The project open, by the name Bitwig shows, and whether it has unsaved changes.
    pub project: String,
    pub modified: bool,
    /// Where Bitwig is playing and how fast; none until the link says.
    pub transport: Option<Transport>,
    /// The tracks in Bitwig's flat list (the first 128), by their place in it. Shared between pictures: an event
    /// without tracks (most are only notes) doesn't copy them.
    pub tracks: Arc<Vec<Track>>,
    /// The track and device selected in Bitwig.
    pub selected: Selected,
    /// The arranger's cue markers, by where they are.
    pub markers: Vec<Marker>,
    /// The tracks playing notes now, with the notes each plays.
    pub notes: Vec<Playing>,
    /// The tracks that show a level now: (place in the list, level from 0 to 1, the louder of left and right).
    pub levels: Vec<(usize, f32)>,
    /// The song's Groove (extension 3.27); none until the link says, and never from an older extension or a Bitwig
    /// that doesn't give it ("groove" is then missing from `sections`).
    pub groove: Option<Groove>,
    /// How many events have come in since the watch started, over every connection: a reader that kept the number
    /// it saw last knows whether anything changed ([`Watch::version`] says the same without taking the picture).
    pub events: u64,
}

/// Bitwig's transport as the feed gives it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Transport {
    pub playing: bool,
    /// Where Bitwig is playing, and where Play starts from, in beats from the arranger's start.
    pub position: f64,
    pub play_start: f64,
    /// In BPM.
    pub tempo: f64,
    /// "4/4".
    pub time_signature: String,
}

/// The song's Groove as the feed gives it: Bitwig's swing for the whole song.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Groove {
    /// Whether the Groove is on: off, the song plays straight whatever the shuffle says.
    pub on: bool,
    /// Bitwig's shuffle amount, from 0 (straight) to 1 (its 100 %).
    pub shuffle: f64,
    /// Which notes the shuffle moves.
    pub rate: GrooveRate,
}

/// The notes a Groove's shuffle moves: every second eighth note, or every second sixteenth.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GrooveRate {
    Eighth,
    #[default]
    Sixteenth,
}

/// A track as the feed gives it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Track {
    /// Its place in Bitwig's flat list, which the link's other commands take.
    pub index: usize,
    /// The ID Bitwig keeps it under, once Bitwig has said.
    pub id: Option<String>,
    pub name: String,
    /// "Instrument", "Audio", "Group", "Effect" or "Master".
    pub kind: String,
    pub group: bool,
    /// "#rrggbb".
    pub color: String,
    pub active: bool,
    pub mute: bool,
    pub solo: bool,
    /// The top of its device chain (the first 16).
    pub devices: Vec<Device>,
}

/// A device on a track, as the feed gives it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Device {
    /// Its place in the chain.
    pub index: usize,
    pub name: String,
    /// The preset loaded in it, "" for none.
    pub preset: String,
    /// Bitwig's kind of device ("instrument", "audio_to_audio", "note_to_audio"...).
    pub kind: String,
    pub plugin: bool,
    pub enabled: bool,
}

/// What's selected in Bitwig.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selected {
    /// The selected track's place in the list; none when no track is selected or it's past the first 128.
    pub track: Option<usize>,
    pub id: Option<String>,
    pub name: String,
    /// The selected device, if any.
    pub device: Option<Device>,
}

/// A cue marker in the arranger.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Marker {
    pub name: String,
    /// Where it is, in beats from the arranger's start.
    pub at: f64,
    /// "#rrggbb".
    pub color: String,
}

/// The notes a track plays now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Playing {
    /// The track's place in the list, and its ID once Bitwig has said.
    pub track: usize,
    pub id: Option<String>,
    /// (key, velocity) by key, each 0 to 127: Bitwig's API gives a playing note's velocity at MIDI 1.0's resolution.
    pub notes: Vec<(u8, u8)>,
}

/// The watch: a thread keeping a [`Live`] up to date from Bitwig, until it's dropped.
pub struct Watch {
    shared: Arc<Shared>,
}

struct Shared {
    live: Mutex<Arc<Live>>,
    /// Bumped with each change to `live`, so a reader can tell without taking the lock.
    version: AtomicU64,
    stop: AtomicBool,
    /// Another handle on the connection in use, to close it from the dropping thread.
    connection: Mutex<Option<TcpStream>>,
    /// Wakes the thread from its wait between tries when the watch is dropped.
    waiting: Mutex<()>,
    wake: Condvar,
}

impl Watch {
    /// Starts watching the link on `port` (the link's is [`super::PORT`]; a test's stand-in has its own).
    pub fn start(port: u16) -> Watch {
        let shared = Arc::new(Shared {
            live: Mutex::new(Arc::new(Live::default())),
            version: AtomicU64::new(0),
            stop: AtomicBool::new(false),
            connection: Mutex::new(None),
            waiting: Mutex::new(()),
            wake: Condvar::new(),
        });
        let theirs = Arc::clone(&shared);
        // A thread that can't start leaves the picture saying it's connecting, which is true enough: nothing comes.
        let _ = thread::Builder::new().name("bitwig watch".into()).spawn(move || {
            // Low priority (rules 12 and 21): the feed must never hold up the screens or the DAW.
            let _ = thread_priority::set_current_thread_priority(thread_priority::ThreadPriority::Min);
            run(port, &theirs);
        });
        Watch { shared }
    }

    /// The song as Bitwig showed it last. Never waits for Bitwig.
    pub fn now(&self) -> Arc<Live> {
        Arc::clone(&lock(&self.shared.live))
    }

    /// A number that goes up each time the picture changes.
    pub fn version(&self) -> u64 {
        self.shared.version.load(Ordering::Acquire)
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        if let Some(connection) = lock(&self.shared.connection).take() {
            let _ = connection.shutdown(Shutdown::Both);
        }
        let _held = lock(&self.shared.waiting);
        self.shared.wake.notify_all();
    }
}

fn run(port: u16, shared: &Shared) {
    while !shared.stop.load(Ordering::Acquire) {
        watch_once(port, shared);
        let wait = match lock(&shared.live).state {
            State::TooOld(_) | State::Refused(_) => RETRY_OLD,
            _ => RETRY,
        };
        // Wait for the next try, unless the watch is dropped meanwhile.
        let held = lock(&shared.waiting);
        if shared.stop.load(Ordering::Acquire) {
            return;
        }
        let _ = shared.wake.wait_timeout(held, wait);
    }
}

/// One connection's worth of watching: until Bitwig closes it, it breaks, or the watch is dropped.
fn watch_once(port: u16, shared: &Shared) {
    let mut link = match Link::connect(port, CONNECT_TIMEOUT) {
        Ok(link) => link,
        Err(_) => {
            publish(shared, |live| *live = fresh(live, State::NoBitwig));
            return;
        }
    };
    if !link.hello().at_least(WATCH_NEEDS) {
        let answering = link.hello().answering();
        publish(shared, |live| *live = fresh(live, State::TooOld(answering)));
        return;
    }
    // Kept before asking, so a drop from now on closes the connection; checked after, as the drop may have come first.
    if let Ok(closer) = link.closer() {
        *lock(&shared.connection) = Some(closer);
    }
    if shared.stop.load(Ordering::Acquire) {
        return;
    }
    let answer = match link.start_watch() {
        Ok(answer) => answer,
        Err(error) => {
            let words = error.to_string();
            publish(shared, |live| *live = fresh(live, State::Refused(words)));
            return;
        }
    };
    let sections: Vec<String> = answer
        .get("sections")
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(Value::as_str).map(str::to_owned).collect())
        .unwrap_or_default();
    publish(shared, |live| *live = Live { sections, ..fresh(live, State::Watching) });
    while let Ok(message) = link.next_event(SILENT) {
        if message.get("event").and_then(Value::as_str) != Some("watch") {
            continue;
        }
        let Some(result) = message.get("result") else { continue };
        publish(shared, |live| {
            merge(live, result);
            live.events += 1;
        });
    }
    lock(&shared.connection).take();
    if !shared.stop.load(Ordering::Acquire) {
        publish(shared, |live| *live = fresh(live, State::Connecting));
    }
}

/// A picture with nothing in it but `state`, keeping the count of events (see [`Live::events`]).
fn fresh(live: &Live, state: State) -> Live {
    Live { state, events: live.events, ..Live::default() }
}

/// Changes the picture (a copy of it: readers holding the old one keep it) and says it changed.
fn publish(shared: &Shared, change: impl FnOnce(&mut Live)) {
    let mut live = lock(&shared.live);
    let mut next = Live::clone(&live);
    change(&mut next);
    *live = Arc::new(next);
    shared.version.fetch_add(1, Ordering::AcqRel);
}

/// Puts the sections an event has into the picture; those it doesn't have stay as they were.
pub(super) fn merge(live: &mut Live, result: &Value) {
    if let Some(p) = result.get("project") {
        live.project = text(p, "name");
        live.modified = flag(p, "modified");
    }
    if let Some(t) = result.get("transport") {
        live.transport = Some(Transport {
            playing: flag(t, "playing"),
            position: number(t, "position"),
            play_start: number(t, "playStart"),
            tempo: number(t, "tempo"),
            time_signature: text(t, "timeSignature"),
        });
    }
    if let Some(list) = result.get("tracks").and_then(Value::as_array) {
        live.tracks = Arc::new(list.iter().filter_map(track).collect());
    }
    if let Some(s) = result.get("selected") {
        live.selected = Selected {
            track: place(s.get("track")),
            id: some_text(s, "id"),
            name: text(s, "name"),
            device: s.get("device").and_then(device),
        };
    }
    if let Some(g) = result.get("groove") {
        // A rate that's neither isn't guessed at: the Groove stays as it was.
        let rate = match g.get("rate").and_then(Value::as_str) {
            Some("1/8") => Some(GrooveRate::Eighth),
            Some("1/16") => Some(GrooveRate::Sixteenth),
            _ => None,
        };
        if let Some(rate) = rate {
            live.groove = Some(Groove { on: flag(g, "on"), shuffle: number(g, "shuffle").clamp(0.0, 1.0), rate });
        }
    }
    if let Some(list) = result.get("markers").and_then(Value::as_array) {
        live.markers = list
            .iter()
            .map(|m| Marker { name: text(m, "name"), at: number(m, "at"), color: text(m, "color") })
            .collect();
    }
    if let Some(list) = result.get("notes").and_then(Value::as_array) {
        live.notes = list
            .iter()
            .filter_map(|p| {
                let notes = p.get("notes")?.as_array()?.iter().filter_map(pair).collect();
                Some(Playing { track: place(p.get("track"))?, id: some_text(p, "id"), notes })
            })
            .collect();
    }
    if let Some(list) = result.get("levels").and_then(Value::as_array) {
        live.levels = list
            .iter()
            .filter_map(|l| {
                let l = l.as_array()?;
                let level = l.get(1)?.as_f64()?.clamp(0.0, 1.0);
                // A level from 0 to 1 fits an f32.
                Some((place(l.first())?, level as f32))
            })
            .collect();
    }
}

fn track(t: &Value) -> Option<Track> {
    Some(Track {
        index: place(t.get("index"))?,
        id: some_text(t, "id"),
        name: text(t, "name"),
        kind: text(t, "type"),
        group: flag(t, "group"),
        color: text(t, "color"),
        active: flag(t, "active"),
        mute: flag(t, "mute"),
        solo: flag(t, "solo"),
        devices: t
            .get("devices")
            .and_then(Value::as_array)
            .map(|d| d.iter().filter_map(device).collect())
            .unwrap_or_default(),
    })
}

fn device(d: &Value) -> Option<Device> {
    if !d.is_object() {
        return None;
    }
    Some(Device {
        index: place(d.get("index")).unwrap_or(0),
        name: text(d, "name"),
        preset: text(d, "preset"),
        kind: text(d, "kind"),
        plugin: flag(d, "plugin"),
        enabled: flag(d, "enabled"),
    })
}

/// A [key, velocity] pair, each 0 to 127.
fn pair(v: &Value) -> Option<(u8, u8)> {
    let v = v.as_array()?;
    let byte = |n: Option<&Value>| n?.as_f64().filter(|n| (0.0..=127.0).contains(n)).map(|n| n as u8);
    Some((byte(v.first())?, byte(v.get(1))?))
}

/// A place in a list: a whole number, 0 or more (the link's JSON may write it 1.0).
fn place(v: Option<&Value>) -> Option<usize> {
    let n = v?.as_f64()?;
    (n >= 0.0 && n.fract() == 0.0 && n < 1e9).then_some(n as usize)
}

fn text(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or_default().to_owned()
}

fn some_text(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_owned)
}

fn flag(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn number(v: &Value, key: &str) -> f64 {
    v.get(key).and_then(Value::as_f64).filter(|n| n.is_finite()).unwrap_or(0.0)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn an_event_changes_only_the_sections_it_has() {
        let mut live = Live { project: "Night Drive".into(), ..Live::default() };
        merge(
            &mut live,
            &json!({
                "notes": [{"track": 2, "id": "bass", "notes": [[60, 100], [64.0, 90]]}],
                "levels": [[2, 0.5], [3, 1.5]],
            }),
        );
        assert_eq!(live.project, "Night Drive");
        assert_eq!(live.notes, vec![Playing { track: 2, id: Some("bass".into()), notes: vec![(60, 100), (64, 90)] }]);
        assert_eq!(live.levels, vec![(2, 0.5), (3, 1.0)]);
        // An empty list is news too: the notes stopped.
        merge(&mut live, &json!({"notes": []}));
        assert!(live.notes.is_empty());
        assert_eq!(live.levels.len(), 2);
    }

    #[test]
    fn reads_the_rest_of_the_sections() {
        let mut live = Live::default();
        merge(
            &mut live,
            &json!({
                "project": {"name": "Night Drive", "modified": true},
                "transport": {"playing": true, "position": 16.5, "playStart": 0, "tempo": 124, "timeSignature": "7/8"},
                "tracks": [{"index": 1, "id": "bass", "name": "Bass", "type": "Instrument", "group": false,
                            "color": "#ff8000", "active": true, "mute": false, "solo": false,
                            "devices": [{"index": 0, "name": "Serum", "preset": "Reese", "kind": "instrument",
                                         "plugin": true, "enabled": true}]},
                           {"name": "no place, left out"}],
                "selected": {"track": 1, "id": "bass", "name": "Bass", "device": null},
                "markers": [{"name": "Chorus", "at": 32, "color": "#ff0000"}],
                "groove": {"on": true, "shuffle": 0.62, "rate": "1/8"},
            }),
        );
        assert_eq!(live.groove, Some(Groove { on: true, shuffle: 0.62, rate: GrooveRate::Eighth }));
        merge(&mut live, &json!({"groove": {"on": false, "shuffle": 7, "rate": "1/16"}}));
        assert_eq!(live.groove, Some(Groove { on: false, shuffle: 1.0, rate: GrooveRate::Sixteenth }));
        merge(&mut live, &json!({"groove": {"on": true, "shuffle": 0.2, "rate": "1/4"}}));
        assert_eq!(live.groove.map(|g| g.on), Some(false), "a rate the app doesn't know leaves the Groove as it was");
        assert!(live.modified);
        assert_eq!(live.transport.as_ref().map(|t| (t.playing, t.position, t.tempo)), Some((true, 16.5, 124.0)));
        assert_eq!(live.tracks.len(), 1);
        assert_eq!(live.tracks[0].devices[0].preset, "Reese");
        assert_eq!(live.selected.track, Some(1));
        assert_eq!(live.selected.device, None);
        assert_eq!(live.markers[0].at, 32.0);
    }

    #[test]
    fn odd_notes_are_passed_over() {
        let mut live = Live::default();
        merge(&mut live, &json!({"notes": [{"track": 1, "notes": [[60, 200], [61], "x", [62, 1]]}, {"notes": []}]}));
        assert_eq!(live.notes, vec![Playing { track: 1, id: None, notes: vec![(62, 1)] }]);
    }
}
