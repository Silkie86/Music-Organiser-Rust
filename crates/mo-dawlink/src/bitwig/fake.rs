//! A stand-in for the Music Organiser link in Bitwig, for tests: no test reaches a real Bitwig (rule 11). It listens
//! on a free port on this computer, greets each connection as extension 3.20 does, and answers `hello`, `newtrack`,
//! `insert`, `pads`, `clip`, `selectedtrack`, `transport`, `stoptracks`, `deleteclip` and `deletetrack` the way the extension does, in its words (`extensions/bitwig-java`'s `Link.java`), on a made-up project:
//! the C# app's stand-in's, "Night Drive", with Drums, Kick, Bass and Master. It handles one request at a time,
//! whichever connection it came on, as the extension makes changes one at a time.
//!
//! This crate's tests have it; another crate's tests get it with the `fake-bitwig` feature:
//!
//! ```toml
//! [dev-dependencies]
//! mo-dawlink = { path = "../mo-dawlink", features = ["fake-bitwig"] }
//! ```
//!
//! Then `FakeBitwig::start()` starts one, `.bitwig()` is where the app finds it (for `send_sound` and the rest), and
//! `.world()` is its Bitwig, to change before a send or look at after one.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use mo_formats::paths::{file_name, stem};
use serde_json::{Map, Value, json};

use super::link::{ascii_json, take_message};
use super::{Bitwig, DRUM_MACHINE_NAME, DeviceId, PORT};

/// The extension's words when "Let the app make changes" is off.
pub const CHANGES_OFF: &str = "Changes are turned off. To let the app change things, turn on \"Let the app make \
                               changes\" in Bitwig: Settings > Controllers > Music Organiser.";

/// How long the app waits for the stand-in: it answers at once, unless a test makes it wait.
const WAIT: Duration = Duration::from_secs(2);

/// The tracks and the devices on each that the extension can reach.
const MAX_TRACKS: usize = 128;
const MAX_DEVICES: usize = 16;

/// The stand-in, listening until it's dropped.
pub struct FakeBitwig {
    port: u16,
    world: Arc<Mutex<World>>,
    stop: Arc<AtomicBool>,
    connections: Arc<Mutex<Vec<TcpStream>>>,
    accepting: Option<JoinHandle<()>>,
}

impl FakeBitwig {
    /// Starts one with the usual project ([`World::default`]).
    pub fn start() -> io::Result<FakeBitwig> {
        FakeBitwig::with(World::default())
    }

    /// Starts one with this Bitwig.
    pub fn with(world: World) -> io::Result<FakeBitwig> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        // So the loop that takes connections sees when to stop.
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let world = Arc::new(Mutex::new(world));
        let stop = Arc::new(AtomicBool::new(false));
        let connections = Arc::new(Mutex::new(Vec::new()));
        let accepting = {
            let (world, stop, connections) = (Arc::clone(&world), Arc::clone(&stop), Arc::clone(&connections));
            thread::Builder::new()
                .name("fake Bitwig".into())
                .spawn(move || accept(&listener, &world, &stop, &connections))?
        };
        Ok(FakeBitwig { port, world, stop, connections, accepting: Some(accepting) })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// The stand-in's Bitwig, to change before a request or look at after one. Requests wait while it's held, so
    /// let go of it (end the statement) before sending anything.
    pub fn world(&self) -> MutexGuard<'_, World> {
        lock(&self.world)
    }

    /// Where the app finds this stand-in, with short waits.
    pub fn bitwig(&self) -> Bitwig {
        Bitwig { port: self.port, connect_timeout: WAIT, timeout: WAIT, change_timeout: WAIT }
    }
}

impl Drop for FakeBitwig {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(accepting) = self.accepting.take() {
            let _ = accepting.join();
        }
        for stream in lock(&self.connections).drain(..) {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }
}

/// The clip launcher slots on a track the link reaches, as its `SCENES`.
const CLIP_SLOTS: usize = 8;

/// What the stand-in does when the app connects.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum OnConnect {
    /// Greets it, as the link does.
    #[default]
    Greet,
    /// Says nothing: something else listening on the port.
    Nothing,
    /// Sends these bytes, and nothing after: something else again, a web server, say.
    Send(Vec<u8>),
    /// Closes the connection at once.
    HangUp,
}

/// The stand-in's Bitwig: the project it has open, what the link says when it greets, and what the app asked for.
#[derive(Clone, Debug)]
pub struct World {
    /// The greeting, but for `project`, `modified` and `changes`, which come from the fields below unless they're
    /// here (to greet with something else than is so). Set "link" to 1 or 2 to be an older script, take out "onlyIn"
    /// to be one that doesn't honour it, or add an "error" to greet as an extension part of which didn't start.
    pub hello: Map<String, Value>,
    /// The project open, by the name Bitwig shows.
    pub project: String,
    pub modified: bool,
    /// "Let the app make changes".
    pub changes: bool,
    /// The project listing, as the link sees it.
    pub tracks: Vec<FakeTrack>,
    /// The selected track's place in the listing.
    pub selected_track: Option<usize>,
    /// The selected device: its track's place in the listing, and its place in the track's chain.
    pub selected_device: Option<(usize, usize)>,
    /// What a new Drum Machine has on every pad: none, or (as a saved default Drum Machine can, Solomon's does) an
    /// empty Sampler, say.
    pub drum_pads: Vec<FakeDevice>,
    /// The device an audio file goes into when the app adds it to a track's chain: Bitwig's "Sampler".
    pub sample_device: String,
    /// Files (by the last part of their path) Bitwig quietly doesn't load.
    pub unloadable: Vec<String>,
    /// What the stand-in does when the app connects.
    pub on_connect: OnConnect,
    /// Whether each message goes with its 4-byte length in front, as Bitwig sends them; without, it's the JSON alone
    /// on a line, as the script once sent it.
    pub lengths: bool,
    /// Commands the stand-in never answers.
    pub silent: Vec<String>,
    /// Commands the stand-in closes the connection on, instead of answering.
    pub hang_up_on: Vec<String>,
    /// Commands answered only after a while, each once: the first request with the command waits that long.
    pub late: Vec<(String, Duration)>,
    /// Brings another project to the front part way through the next request with this command, as if someone
    /// brought their own to the front just after the change began: (command, the project).
    pub switch_during: Option<(String, String)>,
    /// Brings this project to the front once the next kit's samples are on their pads, before they're named: the link
    /// stops there, with the samples in.
    pub switch_while_filling: Option<String>,
    /// Whether the link answers before Bitwig has named a device it added (its name "" in the answer).
    pub unnamed_devices: bool,
    /// Whether Bitwig is too slow to show a Drum Machine's pads to the link, which waits 5 seconds for them when it's
    /// given the Drum Machine's place: it then says the device isn't a Drum Machine, and puts nothing on it.
    pub slow_pads: bool,
    /// Messages sent once, before the next answer: events, or answers to requests long gone.
    pub before_answer: Vec<Value>,
    /// Every request the app sent, as it came (one that wasn't JSON as its text).
    pub requests: Vec<Value>,
    /// Whether Bitwig is playing: a launched clip starts it, `deleteclip` with `transport` stops it.
    pub playing: bool,
    /// The launched clips, as (track's place, slot).
    pub launched: Vec<(usize, usize)>,
    /// The audition clips the link noted (extension 3.19): track ID, slot, and the name the clip came in with.
    pub auditions: Vec<(String, usize, String)>,
    /// The audition tracks the link noted (extension 3.20): track ID, and the name the track came in with.
    pub audition_tracks: Vec<(String, String)>,
    /// How many tracks have been made, for their IDs.
    made: usize,
}

impl Default for World {
    /// "Night Drive" open, changes allowed, extension 3.20 answering on Bitwig 6.1.5, and the C# stand-in's tracks:
    /// Drums (a group), Kick (audio, with EQ+), Bass (an instrument: Serum, then a Compressor, selected) and the
    /// Master (with a Peak Limiter).
    fn default() -> World {
        let hello = json!({
            "link": 3, "product": "Bitwig Studio", "version": "6.1.5", "api": 23, "platform": "WINDOWS",
            "port": PORT, "engine": true, "onlyIn": true, "runtime": "java", "extension": "3.20", "loopback": true,
        });
        let mut serum = FakeDevice::new("Serum", "instrument");
        serum.preset = "Reese".into();
        serum.plugin = true;
        let tracks = vec![
            FakeTrack::new("Drums", "Group", Vec::new()),
            FakeTrack::new("Kick", "Audio", vec![FakeDevice::new("EQ+", "audio-effect")]),
            FakeTrack::new("Bass", "Instrument", vec![serum, FakeDevice::new("Compressor", "audio-effect")]),
            FakeTrack::new("Master", "Master", vec![FakeDevice::new("Peak Limiter", "audio-effect")]),
        ];
        let mut world = World {
            hello: match hello {
                Value::Object(map) => map,
                _ => Map::new(),
            },
            project: "Night Drive".into(),
            modified: false,
            changes: true,
            tracks,
            selected_track: Some(2),
            selected_device: Some((2, 0)),
            drum_pads: Vec::new(),
            sample_device: "Sampler".into(),
            unloadable: Vec::new(),
            on_connect: OnConnect::Greet,
            lengths: true,
            silent: Vec::new(),
            hang_up_on: Vec::new(),
            late: Vec::new(),
            switch_during: None,
            switch_while_filling: None,
            unnamed_devices: false,
            slow_pads: false,
            before_answer: Vec::new(),
            requests: Vec::new(),
            playing: false,
            launched: Vec::new(),
            auditions: Vec::new(),
            audition_tracks: Vec::new(),
            made: 0,
        };
        for track in &mut world.tracks {
            world.made += 1;
            track.id = format!("fake-track-{}", world.made);
        }
        world
    }
}

/// A track in the project listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FakeTrack {
    pub name: String,
    /// "Instrument", "Audio", "Group", "Effect" or "Master".
    pub kind: String,
    /// Its chain.
    pub devices: Vec<FakeDevice>,
    /// The ID Bitwig keeps it under.
    pub id: String,
    /// Its clip launcher slots, from the first: the file each clip was made from, "" for an empty slot.
    pub clips: Vec<String>,
}

impl FakeTrack {
    pub fn new(name: &str, kind: &str, devices: Vec<FakeDevice>) -> FakeTrack {
        FakeTrack { name: name.into(), kind: kind.into(), devices, id: String::new(), clips: Vec::new() }
    }
}

/// A device in a chain, or on a pad.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FakeDevice {
    pub name: String,
    pub preset: String,
    /// Bitwig's word for its type: "instrument", "audio-effect", "note-effect".
    pub kind: String,
    pub plugin: bool,
    /// A Sampler's sample: the path the app gave.
    pub sample: String,
    /// A Drum Machine's pads, by note; none for any other device.
    pub pads: Option<BTreeMap<u8, FakePad>>,
}

impl FakeDevice {
    pub fn new(name: &str, kind: &str) -> FakeDevice {
        FakeDevice { name: name.into(), kind: kind.into(), ..FakeDevice::default() }
    }

    /// A Sampler with this sample on it ("" for none).
    pub fn sampler(sample: &str) -> FakeDevice {
        FakeDevice { sample: sample.into(), ..FakeDevice::new("Sampler", "instrument") }
    }

    /// As the link describes a device it added.
    fn described(&self, index: usize) -> Value {
        json!({
            "index": index, "name": self.name, "preset": self.preset, "kind": self.kind, "plugin": self.plugin,
            "enabled": true,
        })
    }
}

/// One of a Drum Machine's pads.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FakePad {
    pub name: String,
    /// Its chain: a sample dropped on the pad makes a Sampler, first.
    pub devices: Vec<FakeDevice>,
}

impl FakePad {
    /// What the link says is on a pad it leaves alone.
    fn contents(&self, note: u8) -> Value {
        let first = self.devices.first();
        json!({
            "note": note,
            "name": self.name,
            "devices": self.devices.len(),
            "first": first.map(|d| d.name.as_str()).unwrap_or_default(),
            "sample": first.map(|d| file_name(&d.sample)).unwrap_or_default(),
        })
    }

    /// Whether all the pad has is what a Drum Machine preset put there, as the link tells: a Sampler with no sample,
    /// the pad still named as it came (not after a sample), and nothing after it but effects. Such a pad is free.
    fn holds_only_an_empty_sampler(&self) -> bool {
        let Some((first, after)) = self.devices.split_first() else { return false };
        first.name == "Sampler"
            && first.sample.is_empty()
            && (self.name.is_empty() || self.name == first.name)
            && after.iter().all(|d| d.kind == "audio-effect" || d.kind == "audio_to_audio")
    }
}

impl World {
    /// The greeting the link sends: [`World::hello`], with what's open.
    pub fn greeting(&self) -> Value {
        let mut hello = self.hello.clone();
        // A link that couldn't start says only what it knew.
        if !hello.contains_key("error") {
            hello.entry("project").or_insert_with(|| self.project.clone().into());
            hello.entry("modified").or_insert(self.modified.into());
            hello.entry("changes").or_insert(self.changes.into());
        }
        Value::Object(hello)
    }

    /// The command of each request the app sent, in order ("" for one without).
    pub fn commands(&self) -> Vec<String> {
        self.requests.iter().map(|r| r.get("cmd").and_then(Value::as_str).unwrap_or_default().to_owned()).collect()
    }

    /// The track with this name.
    pub fn track(&self, name: &str) -> Option<&FakeTrack> {
        self.tracks.iter().find(|t| t.name == name)
    }

    /// The pads of the first Drum Machine on the track at `track` in the listing.
    pub fn pads(&self, track: usize) -> Option<&BTreeMap<u8, FakePad>> {
        self.tracks.get(track)?.devices.iter().find_map(|d| d.pads.as_ref())
    }

    /// Answers one request: its result, or the link's words for why not.
    fn answer(&mut self, request: &Value) -> Result<Value, String> {
        let Some(map) = request.as_object() else {
            // What JavaScript's request.cmd was: undefined, unless the request is something false.
            let truthy = match request {
                Value::Null => false,
                Value::Bool(b) => *b,
                Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
                Value::String(s) => !s.is_empty(),
                Value::Array(_) | Value::Object(_) => true,
            };
            return Err(format!("Unknown command: {}", if truthy { "undefined".into() } else { js(Some(request)) }));
        };
        let cmd = map.get("cmd");
        let changes = match cmd.and_then(Value::as_str) {
            Some("hello" | "selectedtrack" | "transport") => false,
            Some("newtrack" | "insert" | "pads" | "clip" | "deleteclip" | "deletetrack" | "stoptracks") => true,
            _ => return Err(format!("Unknown command: {}", js(cmd))),
        };
        self.check_allowed(map, changes)?;
        match cmd.and_then(Value::as_str) {
            Some("hello") => match self.hello.get("error").and_then(Value::as_str) {
                Some(error) => Err(error.to_owned()),
                None => Ok(self.greeting()),
            },
            Some("newtrack") => self.new_track(map),
            Some("insert") => self.insert(map),
            Some("clip") => self.add_clip(map),
            Some("selectedtrack") => Ok(self.selected()),
            Some("transport") => Ok(json!({ "playing": self.playing })),
            Some("deleteclip") => self.delete_clip(map),
            Some("deletetrack") => self.delete_track(map),
            Some("stoptracks") => self.stop_tracks(map),
            _ => self.fill_pads(map),
        }
    }

    /// Refuses a request that may not go ahead now: one kept to another project than the one open (`onlyIn`, which
    /// only a link that says it honours it does), or a change while changes are off.
    fn check_allowed(&self, request: &Map<String, Value>, changes: bool) -> Result<(), String> {
        let honours = self.hello.get("onlyIn").and_then(Value::as_bool) == Some(true);
        match request.get("onlyIn") {
            _ if !honours => {}
            None | Some(Value::Null) => {}
            Some(Value::String(only)) if *only == self.project => {}
            Some(Value::String(only)) => {
                return Err(format!(
                    "Bitwig has \"{}\" open, not \"{only}\", so the link stopped before changing anything there.",
                    self.project
                ));
            }
            Some(_) => return Err("onlyIn has to be a project's name.".into()),
        }
        if changes && !self.changes {
            return Err(CHANGES_OFF.into());
        }
        Ok(())
    }

    /// The link checks again before each step of a change after it waited for Bitwig: this is where a test's other
    /// project comes to the front.
    fn next_step(&mut self, cmd: &str, request: &Map<String, Value>) -> Result<(), String> {
        if self.switch_during.as_ref().is_some_and(|(c, _)| c == cmd)
            && let Some((_, project)) = self.switch_during.take()
        {
            self.project = project;
        }
        self.check_allowed(request, true)
    }

    fn new_track(&mut self, request: &Map<String, Value>) -> Result<Value, String> {
        let name = match request.get("name") {
            None | Some(Value::Null) => "",
            Some(Value::String(name)) => name.as_str(),
            Some(_) => return Err("A track's name is text.".into()),
        };
        if let Some(colour) = request.get("color").filter(|c| !c.is_null())
            && !colour.as_str().is_some_and(is_colour)
        {
            return Err("A colour is written #rrggbb.".into());
        }
        // After the last main track: the effect tracks and the master move down one.
        let at = self.tracks.iter().position(|t| t.kind == "Effect" || t.kind == "Master").unwrap_or(self.tracks.len());
        let instruments = self.tracks.iter().filter(|t| t.kind == "Instrument").count();
        self.made += 1;
        let mut track = FakeTrack::new(&format!("Inst {}", instruments + 1), "Instrument", Vec::new());
        track.id = format!("fake-track-{}", self.made);
        self.tracks.insert(at, track);
        self.selected_track = self.selected_track.map(|t| if t >= at { t + 1 } else { t });
        self.selected_device = self.selected_device.map(|(t, d)| if t >= at { (t + 1, d) } else { (t, d) });
        self.modified = true;
        self.next_step("newtrack", request)?;
        let track = self.tracks.get_mut(at).ok_or("Bitwig didn't add the track.")?;
        if !name.is_empty() {
            track.name = name.to_owned();
        }
        let answer = json!({ "index": at, "name": track.name, "id": track.id });
        if request.get("audition").and_then(Value::as_bool) == Some(true) && !track.name.is_empty() {
            self.audition_tracks.push((track.id.clone(), track.name.clone()));
        }
        self.selected_track = Some(at);
        self.selected_device = None;
        Ok(answer)
    }

    /// The selected track, as the link's `selectedTrack` (extension 3.19).
    fn selected(&self) -> Value {
        match self.selected_track.and_then(|t| self.tracks.get(t).map(|track| (t, track))) {
            Some((t, track)) => json!({ "index": t, "id": track.id, "name": track.name, "type": track.kind }),
            None => json!({ "index": null }),
        }
    }

    /// A track by `trackId` when the request gives one (extension 3.9), else by its place (`track`).
    fn requested_track(&self, request: &Map<String, Value>) -> Result<usize, String> {
        match request.get("trackId") {
            None | Some(Value::Null) => self.check_track(request.get("track").unwrap_or(&Value::Null)),
            Some(Value::String(id)) if !id.is_empty() => self
                .tracks
                .iter()
                .position(|t| t.id == *id)
                .ok_or_else(|| format!("There's no track with ID {id} in the project listing.")),
            Some(_) => Err("A track's ID is text.".into()),
        }
    }

    /// Takes out an audition clip the link noted, while it keeps its name, as the link's `deleteClip` (3.19).
    fn delete_clip(&mut self, request: &Map<String, Value>) -> Result<Value, String> {
        let id = match request.get("trackId") {
            Some(Value::String(id)) if !id.is_empty() => id.clone(),
            _ => return Err("Say which track, by its ID.".into()),
        };
        let slot = index(request.get("slot"), CLIP_SLOTS).ok_or_else(|| {
            format!(
                "The app can reach the first {CLIP_SLOTS} clip slots on a track, not slot {}.",
                js(request.get("slot"))
            )
        })?;
        if request.get("transport").and_then(Value::as_bool) == Some(true) {
            self.playing = false;
        }
        let noted = self.auditions.iter().position(|(t, s, _)| *t == id && *s == slot);
        let name = noted.and_then(|n| self.auditions.get(n)).map(|(_, _, name)| name.clone());
        if let Some(n) = noted {
            self.auditions.remove(n);
        }
        let at = self.tracks.iter().position(|t| t.id == id);
        if let Some(t) = at
            && request.get("stop").and_then(Value::as_bool) == Some(true)
        {
            self.launched.retain(|&(lt, _)| lt != t);
        }
        let now =
            at.and_then(|t| self.tracks.get(t)).and_then(|t| t.clips.get(slot)).map(|c| sample_name(c).to_owned());
        let deleted = matches!((&name, &now), (Some(name), Some(now)) if !now.is_empty() && name == now);
        if deleted
            && let Some(t) = at
            && let Some(clip) = self.tracks.get_mut(t).and_then(|t| t.clips.get_mut(slot))
        {
            clip.clear();
            self.modified = true;
        }
        self.next_step("deleteclip", request)?;
        Ok(json!({ "deleted": deleted }))
    }

    /// Stops and takes out an audition track the link noted, while it keeps its name, as the link's `deleteTrack`
    /// (3.20): the tracks after it move up one.
    fn delete_track(&mut self, request: &Map<String, Value>) -> Result<Value, String> {
        let id = match request.get("trackId") {
            Some(Value::String(id)) if !id.is_empty() => id.clone(),
            _ => return Err("Say which track, by its ID.".into()),
        };
        let noted = self.audition_tracks.iter().position(|(t, _)| *t == id).map(|n| self.audition_tracks.remove(n).1);
        let at = self.tracks.iter().position(|t| t.id == id);
        if let Some(t) = at {
            self.launched.retain(|&(lt, _)| lt != t);
        }
        if request.get("transport").and_then(Value::as_bool) == Some(true) {
            self.playing = false;
        }
        let deleted = match (at, &noted) {
            (Some(t), Some(name)) => self.tracks.get(t).is_some_and(|track| track.name == *name),
            _ => false,
        };
        if deleted && let Some(t) = at {
            self.tracks.remove(t);
            let after = |x: usize| {
                if x > t {
                    Some(x - 1)
                } else if x == t {
                    None
                } else {
                    Some(x)
                }
            };
            self.launched = self.launched.iter().filter_map(|&(lt, s)| after(lt).map(|lt| (lt, s))).collect();
            self.selected_track = self.selected_track.and_then(after);
            self.selected_device = self.selected_device.and_then(|(st, d)| after(st).map(|st| (st, d)));
            self.modified = true;
        }
        self.next_step("deletetrack", request)?;
        Ok(json!({ "deleted": deleted }))
    }

    /// Stops the launcher clips on the tracks given by ID, as the link's `stopTracks` (3.16; `transport` from 3.20).
    fn stop_tracks(&mut self, request: &Map<String, Value>) -> Result<Value, String> {
        let Some(Value::Array(ids)) = request.get("trackIds") else {
            return Err("Say which tracks to stop, by their IDs.".into());
        };
        let mut stopped = Vec::new();
        for id in ids {
            let Some(id) = id.as_str() else { return Err("A track's ID is text.".into()) };
            if let Some(t) = self.tracks.iter().position(|track| track.id == id) {
                self.launched.retain(|&(lt, _)| lt != t);
                stopped.push(id.to_owned());
            }
        }
        if request.get("transport").and_then(Value::as_bool) == Some(true) {
            self.playing = false;
        }
        Ok(json!({ "stopped": stopped }))
    }

    /// A file in a track's first empty clip launcher slot (of the first 8 the link reaches), as the link's `addClip`.
    fn add_clip(&mut self, request: &Map<String, Value>) -> Result<Value, String> {
        let at = self.requested_track(request)?;
        let path = given(request.get("path"), "file")?.to_owned();
        let file = file_name(&path);
        let midi = file
            .rsplit_once('.')
            .is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("mid") || ext.eq_ignore_ascii_case("midi"));
        let loads = self.loads(&path) || (midi && !self.unloadable.iter().any(|u| u == file));
        let track = self.tracks.get_mut(at).ok_or("Bitwig didn't add the track.")?;
        let slot = track.clips.iter().position(String::is_empty).unwrap_or(track.clips.len());
        if slot >= CLIP_SLOTS {
            return Err(format!("The track's first {CLIP_SLOTS} clip slots are full."));
        }
        if !loads {
            return Err(format!("Bitwig didn't make a clip from {path}."));
        }
        if slot == track.clips.len() {
            track.clips.push(path);
        } else if let Some(empty) = track.clips.get_mut(slot) {
            *empty = path;
        }
        let id = track.id.clone();
        self.next_step("clip", request)?;
        if request.get("audition").and_then(Value::as_bool) == Some(true) && !id.is_empty() {
            let name = self.tracks.get(at).and_then(|t| t.clips.get(slot)).map(|c| sample_name(c).to_owned());
            self.auditions.push((id, slot, name.unwrap_or_default()));
        }
        if request.get("launch").and_then(Value::as_bool) == Some(true) {
            self.launched.push((at, slot));
            self.playing = true;
            return Ok(json!({ "slot": slot, "launched": true }));
        }
        Ok(json!({ "slot": slot }))
    }

    fn insert(&mut self, request: &Map<String, Value>) -> Result<Value, String> {
        let at = request.get("at").filter(|a| !a.is_null());
        let start = at.and_then(Value::as_str) == Some("start");
        if at.is_some() && !start && at.and_then(Value::as_str) != Some("end") {
            return Err("Insert at the \"start\" or the \"end\" of the chain.".into());
        }
        let given_track = match request.get("track").filter(|t| !t.is_null()) {
            None if start => return Err("Say which track's chain to insert at the start of.".into()),
            None => None,
            Some(t) => Some(self.check_track(t)?),
        };
        let track = match given_track {
            Some(t) => t,
            None => self.selected_track.filter(|&t| t < self.tracks.len()).ok_or("No track is selected in Bitwig.")?,
        };
        let device = match request.get("kind").and_then(Value::as_str) {
            // The stand-in has no plugins.
            Some("vst3" | "clap") => given(request.get("plugin"), "plugin ID").map(|_| None)?,
            Some("vst2") if request.get("plugin").is_some_and(Value::is_number) => None,
            Some("vst2") => return Err("A VST2 plugin's ID is a number.".into()),
            Some("bitwig") => {
                let text = given(request.get("device"), "device ID")?;
                let id = DeviceId::parse(text).ok_or_else(|| {
                    format!("A Bitwig device's ID is a UUID, such as {}, not {text}.", DeviceId::SAMPLER)
                })?;
                self.bitwig_device(id)
            }
            Some("file") => {
                let path = given(request.get("path"), "file")?;
                if self.loads_preset(path) {
                    // A preset loads as the device it was saved from; the stand-in calls it "Polymer".
                    Some(FakeDevice {
                        preset: sample_name(path).to_owned(),
                        ..FakeDevice::new("Polymer", "instrument")
                    })
                } else {
                    self.loads(path).then(|| FakeDevice {
                        name: self.sample_device.clone(),
                        preset: sample_name(path).to_owned(),
                        ..FakeDevice::sampler(path)
                    })
                }
            }
            _ => {
                let kind = js(request.get("kind"));
                return Err(format!("Can't insert a \"{kind}\": use vst3, clap, vst2, bitwig or file."));
            }
        };
        // Bitwig quietly does nothing for what it can't use.
        let Some(device) = device else {
            return Ok(match given_track {
                Some(_) => json!({ "added": false, "index": null, "device": null }),
                None => json!({}),
            });
        };
        let chain = &mut self.tracks.get_mut(track).ok_or("No track is selected in Bitwig.")?.devices;
        let index = if start { 0 } else { chain.len() };
        chain.insert(index, device.clone());
        self.modified = true;
        self.next_step("insert", request)?;
        Ok(match given_track {
            Some(_) => {
                let mut shown = device.described(index);
                if self.unnamed_devices {
                    shown["name"] = "".into();
                }
                json!({ "added": true, "index": index, "device": shown })
            }
            None => json!({}),
        })
    }

    fn fill_pads(&mut self, request: &Map<String, Value>) -> Result<Value, String> {
        let list = request.get("pads").and_then(Value::as_array).filter(|l| !l.is_empty()).ok_or("No pads given.")?;
        let mut pads: Vec<(u8, &str, &str)> = Vec::new();
        let mut notes = BTreeSet::new();
        for item in list {
            let note = item
                .as_object()
                .and_then(|pad| index(pad.get("note"), 128))
                .and_then(|n| u8::try_from(n).ok())
                .ok_or("A pad's note is a MIDI note from 0 to 127.")?;
            if !notes.insert(note) {
                return Err(format!("Two samples for note {note}."));
            }
            let path = given(item.get("path"), "sample")?;
            let name = match item.get("name") {
                None | Some(Value::Null) => "",
                Some(Value::String(name)) => name.as_str(),
                Some(_) => return Err("A pad's name is text.".into()),
            };
            pads.push((note, path, name));
        }
        let (track, device) = match request.get("track").filter(|t| !t.is_null()) {
            Some(t) => {
                let track = self.check_track(t)?;
                let d = request.get("device").filter(|d| !d.is_null()).cloned().unwrap_or(json!(0));
                let device = index(Some(&d), MAX_DEVICES)
                    .filter(|&d| self.device(track, d).is_some())
                    .ok_or_else(|| format!("Track {track} has no device {}.", js(Some(&d))))?;
                self.selected_track = Some(track);
                self.selected_device = Some((track, device));
                self.next_step("pads", request)?;
                if self.slow_pads || !self.device(track, device).is_some_and(|d| d.pads.is_some()) {
                    return Err(format!("Device {device} on track {track} isn't a Drum Machine."));
                }
                (track, device)
            }
            None => {
                let Some((track, device)) = self.selected_device.filter(|&(t, d)| self.device(t, d).is_some()) else {
                    return Err("No device is selected in Bitwig. Select a Drum Machine and send the kit again.".into());
                };
                self.next_step("pads", request)?;
                if !self.device(track, device).is_some_and(|d| d.pads.is_some()) {
                    return Err("The selected device isn't a Drum Machine. Select one and send the kit again.".into());
                }
                (track, device)
            }
        };
        let loads: Vec<bool> = pads.iter().map(|&(_, path, _)| self.loads(path)).collect();
        let (mut filled, mut replaced, mut skipped, mut failed, mut taken) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
        {
            let machine = self.pads_of(track, device)?;
            for (&(note, path, _), loads) in pads.iter().zip(loads) {
                let pad = machine.entry(note).or_default();
                let free = pad.devices.is_empty();
                if !free && !pad.holds_only_an_empty_sampler() {
                    skipped.push(note);
                    taken.push(pad.contents(note));
                    continue;
                }
                // A file Bitwig can't load never shows on the pad: the link gives up waiting for it.
                if !loads {
                    failed.push(note);
                    continue;
                }
                if free {
                    // Bitwig names a pad after the sample dropped on it.
                    pad.name = sample_name(path).to_owned();
                    pad.devices.push(FakeDevice::sampler(path));
                } else {
                    replaced.push(note);
                    if let Some(first) = pad.devices.first_mut() {
                        *first = FakeDevice::sampler(path);
                    }
                }
                filled.push(note);
            }
            // Pads nothing went on stay as they were: empty.
            machine.retain(|_, pad| !pad.devices.is_empty());
        }
        self.modified = true;
        // The link waits for the pads to take their samples, and checks again before it names them.
        if let Some(project) = self.switch_while_filling.take() {
            self.project = project;
        }
        self.check_allowed(request, true)?;
        let machine = self.pads_of(track, device)?;
        for &(note, _, name) in &pads {
            if let Some(pad) = machine.get_mut(&note).filter(|_| !name.is_empty() && filled.contains(&note)) {
                pad.name = name.to_owned();
            }
        }
        Ok(json!({ "filled": filled, "replaced": replaced, "skipped": skipped, "failed": failed, "taken": taken }))
    }

    /// The pads of the Drum Machine at `device` on `track`.
    fn pads_of(&mut self, track: usize, device: usize) -> Result<&mut BTreeMap<u8, FakePad>, String> {
        self.tracks
            .get_mut(track)
            .and_then(|t| t.devices.get_mut(device))
            .and_then(|d| d.pads.as_mut())
            .ok_or_else(|| "The selected device isn't a Drum Machine. Select one and send the kit again.".to_owned())
    }

    /// One of Bitwig's own devices, by its ID: the stand-in knows the Drum Machine, the Sampler and Note Transpose.
    fn bitwig_device(&self, id: DeviceId) -> Option<FakeDevice> {
        match id {
            DeviceId::DRUM_MACHINE => {
                let pad = (!self.drum_pads.is_empty()).then(|| FakePad {
                    name: self.drum_pads.first().map(|d| d.name.clone()).unwrap_or_default(),
                    devices: self.drum_pads.clone(),
                });
                let pads = (0..=127u8).filter_map(|note| Some((note, pad.clone()?))).collect();
                Some(FakeDevice { pads: Some(pads), ..FakeDevice::new(DRUM_MACHINE_NAME, "instrument") })
            }
            DeviceId::SAMPLER => Some(FakeDevice::sampler("")),
            DeviceId::NOTE_TRANSPOSE => Some(FakeDevice::new("Note Transpose", "note-effect")),
            _ => None,
        }
    }

    /// Whether Bitwig loads this file as a sample: audio it can read, and not one a test said it doesn't.
    fn loads(&self, path: &str) -> bool {
        let file = file_name(path);
        let audio = file.rsplit_once('.').is_some_and(|(_, ext)| {
            ["wav", "aif", "aiff", "flac", "mp3", "ogg"].iter().any(|a| ext.eq_ignore_ascii_case(a))
        });
        audio && !self.unloadable.iter().any(|u| u == file)
    }

    /// Whether Bitwig loads this file as a preset: a `.bwpreset` a test didn't say it doesn't.
    fn loads_preset(&self, path: &str) -> bool {
        let file = file_name(path);
        let preset = file.rsplit_once('.').is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("bwpreset"));
        preset && !self.unloadable.iter().any(|u| u == file)
    }

    fn device(&self, track: usize, device: usize) -> Option<&FakeDevice> {
        self.tracks.get(track)?.devices.get(device)
    }

    fn check_track(&self, t: &Value) -> Result<usize, String> {
        index(Some(t), MAX_TRACKS)
            .filter(|&t| t < self.tracks.len())
            .ok_or_else(|| format!("There's no track {} in the project listing.", js(Some(t))))
    }
}

/// Takes the app's connections, each on a thread of its own, until the stand-in stops.
fn accept(listener: &TcpListener, world: &Arc<Mutex<World>>, stop: &AtomicBool, connections: &Mutex<Vec<TcpStream>>) {
    while !stop.load(Ordering::Relaxed) {
        let Ok((stream, _)) = listener.accept() else {
            thread::sleep(Duration::from_millis(5));
            continue;
        };
        // On some systems a connection takes the listener's non-blocking.
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_nodelay(true);
        if let Ok(copy) = stream.try_clone() {
            lock(connections).push(copy);
        }
        let world = Arc::clone(world);
        let _ = thread::Builder::new().name("fake Bitwig connection".into()).spawn(move || serve(stream, &world));
    }
}

/// What the stand-in does with one request.
enum Reply {
    /// The answer, after any messages a test asked to come first.
    Answer(Vec<Value>),
    Nothing,
    HangUp,
}

/// One connection: the greeting, then each request answered in turn, until the app or the stand-in closes it.
fn serve(mut stream: TcpStream, world: &Mutex<World>) {
    let (on_connect, greeting, lengths) = {
        let w = lock(world);
        (w.on_connect.clone(), w.greeting(), w.lengths)
    };
    let greeted = match on_connect {
        OnConnect::Greet => write_message(&mut stream, &json!({ "event": "hello", "result": greeting }), lengths),
        OnConnect::Nothing => Ok(()),
        OnConnect::Send(bytes) => stream.write_all(&bytes),
        OnConnect::HangUp => {
            let _ = stream.shutdown(Shutdown::Both);
            return;
        }
    };
    if greeted.is_err() {
        return;
    }
    let (mut pending, mut scanned) = (Vec::new(), 0);
    let mut chunk = [0u8; 16 * 1024];
    loop {
        let n = match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        pending.extend_from_slice(chunk.get(..n).unwrap_or_default());
        loop {
            // Read as the app reads Bitwig's messages.
            let body = match take_message(&mut pending, &mut scanned) {
                Ok(Some(body)) => body,
                Ok(None) => break,
                Err(_) => {
                    let _ = stream.shutdown(Shutdown::Both);
                    return;
                }
            };
            let (reply, wait) = handle(&mut lock(world), &String::from_utf8_lossy(&body));
            match reply {
                Reply::Answer(messages) => {
                    if let Some(wait) = wait {
                        thread::sleep(wait);
                    }
                    if messages.iter().any(|m| write_message(&mut stream, m, lengths).is_err()) {
                        return;
                    }
                }
                Reply::Nothing => {}
                Reply::HangUp => {
                    let _ = stream.shutdown(Shutdown::Both);
                    return;
                }
            }
        }
    }
}

/// Answers one request, as the link does, and says how long to wait first.
fn handle(world: &mut World, text: &str) -> (Reply, Option<Duration>) {
    let Ok(request) = serde_json::from_str::<Value>(text) else {
        world.requests.push(Value::String(text.to_owned()));
        let start: String = text.chars().take(80).collect();
        let error = format!("The request isn't JSON: {start}");
        return (Reply::Answer(vec![json!({ "id": null, "ok": false, "error": error })]), None);
    };
    world.requests.push(request.clone());
    let cmd = request.get("cmd").and_then(Value::as_str).unwrap_or_default();
    if world.silent.iter().any(|c| c == cmd) {
        return (Reply::Nothing, None);
    }
    if world.hang_up_on.iter().any(|c| c == cmd) {
        return (Reply::HangUp, None);
    }
    let wait = world.late.iter().position(|(c, _)| c == cmd).map(|at| world.late.remove(at).1);
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let answer = match world.answer(&request) {
        Ok(result) => json!({ "id": id, "ok": true, "result": result }),
        Err(error) => json!({ "id": id, "ok": false, "error": error }),
    };
    let mut messages = std::mem::take(&mut world.before_answer);
    messages.push(answer);
    (Reply::Answer(messages), wait)
}

/// A message as the extension sends it: the JSON in plain ASCII and a newline, with the length in front (as Bitwig
/// puts it) unless the test said not to.
fn write_message(stream: &mut TcpStream, message: &Value, lengths: bool) -> io::Result<()> {
    let mut body = ascii_json(message).into_bytes();
    body.push(b'\n');
    let length = u32::try_from(body.len()).map_err(io::Error::other)?;
    let mut frame = Vec::with_capacity(4 + body.len());
    if lengths {
        frame.extend_from_slice(&length.to_be_bytes());
    }
    frame.extend_from_slice(&body);
    stream.write_all(&frame)
}

/// The name Bitwig gives a sample dropped on a pad, or a Sampler it loads: the file's name without its ending, whichever
/// way the path's folders are written.
fn sample_name(path: &str) -> &str {
    stem(file_name(path))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // A test that failed while holding the world leaves it as it was.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A text the link says it needs, not empty.
fn given<'a>(value: Option<&'a Value>, what: &str) -> Result<&'a str, String> {
    value.and_then(Value::as_str).filter(|s| !s.is_empty()).ok_or_else(|| format!("No {what} given."))
}

/// A place in a list of `count`: a whole number from 0, as the link reads one.
fn index(value: Option<&Value>, count: usize) -> Option<usize> {
    let f = value?.as_f64()?;
    (f >= 0.0 && f < count as f64 && f.fract() == 0.0).then_some(f as usize)
}

fn is_colour(text: &str) -> bool {
    text.len() == 7 && text.starts_with('#') && text.bytes().skip(1).all(|b| b.is_ascii_hexdigit())
}

/// A value as JavaScript's String() writes it, as the link's messages show what a request asked for.
fn js(value: Option<&Value>) -> String {
    match value {
        None => "undefined".into(),
        Some(Value::Null) => "null".into(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Number(n)) => match n.as_f64() {
            Some(f) if f.fract() == 0.0 && f.abs() < 9.0e15 => format!("{}", f as i64),
            _ => n.to_string(),
        },
        Some(Value::Object(_)) => "[object Object]".into(),
        Some(Value::Array(items)) => {
            items.iter().map(|i| if i.is_null() { String::new() } else { js(Some(i)) }).collect::<Vec<_>>().join(",")
        }
    }
}
