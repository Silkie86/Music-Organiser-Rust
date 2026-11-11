//! The connection to the Music Organiser link in Bitwig (the C# app's `BitwigLink`).
//!
//! The link (the Java extension, or the JavaScript script it took over from) listens on port 19791 on this computer.
//! Each message is JSON with a 4-byte big-endian length in front, which is how Bitwig's remote connections frame them;
//! the link also ends each of its messages with a newline, so one that comes without the length still reads. When the
//! app connects, the link greets it with `{"event": "hello", "result": {...}}`. Requests are
//! `{"id": 1, "cmd": "...", ...}`, answered with `{"id": 1, "ok": true, "result": ...}` or
//! `{"id": 1, "ok": false, "error": "..."}`, the reason in words to show as they are. The whole protocol is in the C#
//! app's `extensions/README.md` ("Bitwig link").
//!
//! A [`Link`] sends one request at a time and waits for its answer. Something that wants two things from Bitwig at
//! once opens two links: the link in Bitwig takes several connections, and makes the changes they ask for one at a
//! time, in the order they came.

use std::fmt::Write as _;
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use super::{DeviceId, Error, Pad};

/// The port the link listens on, on this computer only.
pub const PORT: u16 = 19791;

/// The oldest link that takes tracks, devices and kits; a version 1 script can only be read. The Java extension is
/// version 3, the script 2.
pub const LINK_VERSION: i64 = 2;

/// How long to wait for the link to take the connection, and then for its greeting.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// How long to wait for an answer that only reads.
pub const TIMEOUT: Duration = Duration::from_secs(10);
/// How long to wait for each change Bitwig makes and the link waits to see (a new track, a device, a kit's pads):
/// the link gives up on each of its steps after 5 seconds, and some changes take a few.
pub const CHANGE_TIMEOUT: Duration = Duration::from_secs(30);

/// The longest message the app takes, as the C# app and the extension do: anything longer is a broken stream.
pub(super) const MAX_MESSAGE: usize = 64 << 20;

/// The longest wait for anything, so a deadline never runs past what the clock can count.
const LONGEST_WAIT: Duration = Duration::from_secs(3600);

/// The extension version from which `insert` and `pads` find a track by its ID too, as `clip` does.
pub const TRACK_IDS_EVERYWHERE: &str = "3.21";

/// The commands that change or select something in Bitwig's project, as the link lists them: each carries
/// [`Link::only_in`] when it's set. Not `recordabort`, which must reach the link whatever project is in front.
const PROJECT_COMMANDS: [&str; 16] = [
    "set",
    "insert",
    "newtrack",
    "clip",
    "pads",
    "transpose",
    "action",
    "recorder",
    "arrange",
    "select",
    "filetrack",
    "record",
    "probe",
    "stoptracks",
    "deleteclip",
    "deletetrack",
];

/// How taking an audition clip out went ([`Link::delete_audition_clip`], [`super::Bitwig::stop_playing`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TakenOut {
    /// Whether the clip was taken out: false when it was renamed (so it's yours now) or is gone already.
    pub deleted: bool,
    /// For a clip that played with its song's bars going round: whether Bitwig showed the arranger's loop and the
    /// play start back as the link found them (as far as they were still the link's to put back). None for a clip
    /// that played alone, and when its loop had gone back already.
    pub loop_back: Option<bool>,
}

/// What the link's record job answered ([`Link::record_clip`]): it plays a launcher clip once with Bitwig's arranger
/// record on, which puts the clip on its track's arranger timeline, then sets Bitwig back as it found it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Recorded {
    /// The job's own verdict: "recorded" (it heard the clip back from the arranger), "notChecked" (it recorded, and
    /// wasn't asked to play it back), "nothing", "cantTell", "refused", "didntStart", "aborted", "abandoned" or
    /// "failed".
    pub decided: String,
    /// How the clip's playing ended: "done" (at its end), "cut" (the link ended it just past its end), "timedOut"
    /// or "stoppedEarly" (before its end); "" when it never played.
    pub ended: String,
    /// Whether the link saw the clip play.
    pub clip_played: bool,
    /// The settings Bitwig didn't take for the recording, by the link's names ("pre-roll", "arm"): it recorded
    /// nothing then.
    pub refused: Vec<String>,
    /// Why it stopped part way or failed, in the link's words; "" when it didn't say.
    pub why: String,
    /// Whether the job took the launcher clip out once it was recorded.
    pub launcher_clip_deleted: bool,
    /// The settings Bitwig didn't show back as they were, by the link's names ("master mute"). None when the job
    /// didn't wait to see them back: another project came to the front, or a step failed.
    pub not_back: Option<Vec<String>>,
}

impl Recorded {
    fn from_json(v: &Value) -> Option<Recorded> {
        let decided = v.get("decided")?.as_str()?.to_owned();
        let names = |list: Option<&Value>| -> Option<Vec<String>> {
            Some(list?.as_array()?.iter().filter_map(|name| name.as_str().map(str::to_owned)).collect())
        };
        let played = v.get("played").filter(|p| p.is_object());
        let why = ["abortWhy", "error"].iter().map(|key| text(v, key)).find(|words| !words.is_empty());
        Some(Recorded {
            decided,
            ended: played.map(|p| text(p, "ended")).unwrap_or_default(),
            clip_played: v.get("seen").is_some_and(|seen| flag(seen, "clipPlayed")),
            refused: names(v.get("refused")).unwrap_or_default(),
            why: why.unwrap_or_default(),
            launcher_clip_deleted: flag(v, "launcherClipDeleted"),
            not_back: names(v.get("after").and_then(|after| after.get("restoreFailed"))),
        })
    }
}

/// What the link in Bitwig says when the app connects (and to `hello`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hello {
    /// The protocol's version: 3 from the Java extension, 2 from the script, 1 from a script too old to send to.
    pub link: i64,
    /// "Bitwig Studio".
    pub product: String,
    /// Bitwig's version, "6.1.5".
    pub version: String,
    /// The controller API version Bitwig offers.
    pub api: i64,
    pub platform: String,
    pub port: i64,
    /// The project open in Bitwig, by the name Bitwig shows; "" when the greeting couldn't say.
    pub project: String,
    /// Whether it has unsaved changes.
    pub modified: bool,
    /// Whether Bitwig's audio engine is on.
    pub engine: bool,
    /// Whether "Let the app make changes" is on in the controller's settings in Bitwig.
    pub changes: bool,
    /// Whether the link honours a request's `onlyIn` ([`Link::only_in`]); older scripts don't.
    pub only_in: bool,
    /// "java" when the Java extension answers; "" for the script.
    pub runtime: String,
    /// The extension's version ("3.18"); "" for the script.
    pub extension: String,
    /// The file Bitwig loaded the extension from, when it can tell (extension 3.4).
    pub loaded_from: String,
    /// Whether the extension's own MIDI has come back to it through its loopback (extension 3.7), which Save from the
    /// app needs; none when the link doesn't say.
    pub loopback: Option<bool>,
    /// Whether this Bitwig let the extension reach the arranger's loop and the play start (extension 3.22), which a
    /// part played with its song's bars needs ([`Link::add_audition_clip`]); none when the link doesn't say.
    pub song_loop: Option<bool>,
    /// Why part of the extension didn't start, when it greeted with only what it knew (no project then).
    pub error: Option<String>,
}

impl Hello {
    /// The greeting as the link sends it; anything missing or of another kind reads as nothing.
    pub fn from_json(v: &Value) -> Hello {
        Hello {
            link: whole(v.get("link")).unwrap_or(0),
            product: text(v, "product"),
            version: text(v, "version"),
            api: whole(v.get("api")).unwrap_or(0),
            platform: text(v, "platform"),
            port: whole(v.get("port")).unwrap_or(0),
            project: text(v, "project"),
            modified: flag(v, "modified"),
            engine: flag(v, "engine"),
            changes: flag(v, "changes"),
            only_in: flag(v, "onlyIn"),
            runtime: text(v, "runtime"),
            extension: text(v, "extension"),
            loaded_from: text(v, "loadedFrom"),
            loopback: v.get("loopback").and_then(Value::as_bool),
            song_loop: v.get("songLoop").and_then(Value::as_bool),
            error: v.get("error").and_then(Value::as_str).filter(|e| !e.is_empty()).map(str::to_owned),
        }
    }

    /// What answers in Bitwig, in words: "the Music Organiser extension 3.18", or "the Music Organiser script".
    pub fn answering(&self) -> String {
        if self.runtime == "java" {
            format!("the Music Organiser extension {}", self.extension).trim_end().to_owned()
        } else {
            "the Music Organiser script".to_owned()
        }
    }

    /// Whether the extension answering is `needs` ("3.21") or later; the script never is.
    pub fn at_least(&self, needs: &str) -> bool {
        fn parts(v: &str) -> Option<(u32, u32)> {
            let (major, minor) = v.trim().split_once('.')?;
            Some((major.parse().ok()?, minor.split('.').next()?.parse().ok()?))
        }
        self.runtime == "java" && matches!((parts(&self.extension), parts(needs)), (Some(has), Some(n)) if has >= n)
    }

    /// Whether the app may send things in: the link started, is new enough, and has changes allowed. Says why not.
    pub fn can_send(&self) -> Result<(), Error> {
        if let Some(why) = &self.error {
            return Err(Error::NotStarted(why.clone()));
        }
        if self.link < LINK_VERSION {
            return Err(Error::OldLink);
        }
        if !self.changes {
            return Err(Error::ChangesOff);
        }
        Ok(())
    }
}

/// A track Bitwig added ([`Link::new_track`]): its place in the project listing (which the other changes take), its
/// name, and the ID Bitwig keeps it under (the extension 3.9 and later; none from the script).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewTrack {
    pub index: usize,
    pub name: String,
    pub id: Option<String>,
}

/// The track selected in Bitwig ([`Link::selected_track`], extension 3.19): its place in the project listing, the ID
/// Bitwig keeps it under (when it says), its name and Bitwig's word for its kind ("Instrument", "Audio"...).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectedTrack {
    pub index: usize,
    pub id: Option<String>,
    pub name: String,
    pub kind: String,
}

/// A device in a track's chain, as Bitwig shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Device {
    /// "Sampler", "Drum Machine", a plugin's name.
    pub name: String,
    /// The preset loaded on it, if Bitwig names one.
    pub preset: String,
    /// Bitwig's word for its type ("instrument", "audio-effect"; older Bitwig said "note_to_audio").
    pub kind: String,
    pub plugin: bool,
    pub enabled: bool,
}

/// What Bitwig added to a track's chain ([`Link::insert_file`], [`Link::insert_device`]). Bitwig quietly ignores a
/// file or ID it can't use, so `added` is false then.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Inserted {
    pub added: bool,
    /// Where the new device is in the chain.
    pub index: Option<usize>,
    /// The new device, once Bitwig showed it (none when it didn't in time).
    pub device: Option<Device>,
}

/// What became of each pad of a kit ([`Link::fill_pads`]), by MIDI note.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PadsFilled {
    /// The pads that got their sample.
    pub filled: Vec<u8>,
    /// Those of the filled that went in place of an empty Sampler a Drum Machine preset put on the pad (extension
    /// 3.3).
    pub replaced: Vec<u8>,
    /// Pads that already had something on them, left alone.
    pub skipped: Vec<u8>,
    /// Pads Bitwig didn't fill.
    pub failed: Vec<u8>,
    /// What was on each skipped pad (extension 3.2).
    pub taken: Vec<PadContents>,
}

/// What Bitwig showed on a pad the link left alone: its name, how many devices, the first one's name and that one's
/// sample's file name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PadContents {
    pub note: u8,
    pub name: String,
    pub devices: i64,
    pub first: String,
    pub sample: String,
}

/// A connection to the Music Organiser link in Bitwig. Dropping it closes the connection; a change still waiting in
/// Bitwig behind others is then left out (extension 3.18), while one Bitwig has started goes on to the end. An answer
/// that doesn't come in time closes it too: on Windows, a read that ran out of time leaves a connection in a state it
/// mustn't be used in (it can lose what comes next), so a link that gave up waiting connects again.
#[derive(Debug)]
pub struct Link {
    stream: TcpStream,
    /// What Bitwig has sent that doesn't make a whole message yet.
    pending: Vec<u8>,
    /// How far into `pending` a message that came without its length has been searched for its end.
    scanned: usize,
    last_id: u64,
    hello: Hello,
    open: bool,
    only_in: Option<String>,
    /// How long to wait for an answer that only reads ([`TIMEOUT`]).
    pub timeout: Duration,
    /// How long to wait for each change ([`CHANGE_TIMEOUT`]).
    pub change_timeout: Duration,
}

/// Why reading from Bitwig stopped.
pub(super) enum Failure {
    TimedOut,
    Closed,
    /// What came isn't the link's messages.
    Garbled(String),
    /// The system's words for why the connection broke.
    Broken(String),
}

impl Link {
    /// Connects to the link on `port` on this computer and waits for its greeting, up to `timeout` for each.
    pub fn connect(port: u16, timeout: Duration) -> Result<Link, Error> {
        let timeout = timeout.clamp(Duration::from_millis(1), LONGEST_WAIT);
        // The link may listen on either loopback address.
        let stream = [IpAddr::V4(Ipv4Addr::LOCALHOST), IpAddr::V6(Ipv6Addr::LOCALHOST)]
            .into_iter()
            .find_map(|ip| TcpStream::connect_timeout(&SocketAddr::new(ip, port), timeout).ok())
            .ok_or(Error::NotRunning)?;
        // Each request is one small write, waited on: send it at once.
        let _ = stream.set_nodelay(true);
        let mut link = Link {
            stream,
            pending: Vec::new(),
            scanned: 0,
            last_id: 0,
            hello: Hello::default(),
            open: true,
            only_in: None,
            timeout: TIMEOUT,
            change_timeout: CHANGE_TIMEOUT,
        };
        // The link greets every connection, which also shows it's the Music Organiser link listening.
        let deadline = Instant::now() + timeout;
        loop {
            match link.next_message(deadline) {
                Ok(message) => {
                    if message.get("event").and_then(Value::as_str) == Some("hello")
                        && let Some(result) = message.get("result").filter(|r| r.is_object())
                    {
                        link.hello = Hello::from_json(result);
                        return Ok(link);
                    }
                    // Anything else before the greeting isn't for the app.
                }
                Err(Failure::TimedOut | Failure::Garbled(_)) => return Err(Error::NotTheLink { port }),
                Err(Failure::Closed) => return Err(Error::Closed),
                Err(Failure::Broken(why)) => return Err(Error::Lost(why)),
            }
        }
    }

    /// What the link said when it last greeted the app (or answered [`Link::refresh`]).
    pub fn hello(&self) -> &Hello {
        &self.hello
    }

    /// Asks the link again what Bitwig has open (`hello`), and keeps the answer. The link doesn't note these in its
    /// reports, so asking every couple of seconds (to show whether Bitwig is there) costs nothing.
    pub fn refresh(&mut self) -> Result<&Hello, Error> {
        let answer = self.request(ask("hello", []), self.timeout)?;
        if !answer.is_object() {
            return Err(Error::Unexpected);
        }
        self.hello = Hello::from_json(&answer);
        Ok(&self.hello)
    }

    /// Whether the connection is still open: false once Bitwig closed it or it broke.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The project each change goes to, when set: every request that changes or selects something then carries it
    /// (`onlyIn`), and the link refuses the request, or stops a change part way, unless Bitwig has that project open.
    /// Bitwig makes some changes in steps and only the link sees each, so this keeps a change in the project it was
    /// meant for when another one comes to the front meanwhile.
    pub fn only_in(&self) -> Option<&str> {
        self.only_in.as_deref()
    }

    /// Sets (or, with none, clears) [`Link::only_in`].
    pub fn set_only_in(&mut self, project: Option<String>) {
        self.only_in = project;
    }

    /// Keeps every change from now on to the project Bitwig had open when the link last greeted the app (see
    /// [`Link::only_in`]), when the link can do that and Bitwig named the project; otherwise changes go to whichever
    /// project is open, as they did before the link could keep them to one.
    pub fn stay_in_open_project(&mut self) {
        let pinned = self.hello.only_in && !self.hello.project.is_empty();
        self.only_in = pinned.then(|| self.hello.project.clone());
    }

    /// Sends any command, with `fields` (a JSON object, or null for none) added to it, and returns Bitwig's answer as
    /// it came: for commands this crate has no call for yet. A change waits up to [`Link::change_timeout`], anything
    /// else up to [`Link::timeout`].
    pub fn command(&mut self, cmd: &str, fields: Value) -> Result<Value, Error> {
        let mut request = match fields {
            Value::Object(map) => map,
            Value::Null => Map::new(),
            _ => return Err(Error::Invalid(format!("A request's fields are a JSON object, not {fields}."))),
        };
        request.insert("cmd".into(), cmd.into());
        let timeout = if PROJECT_COMMANDS.contains(&cmd) { self.change_timeout } else { self.timeout };
        self.request(request, timeout)
    }

    /// Adds an instrument track after the project's last track (before the effect tracks), names it and selects it.
    pub fn new_track(&mut self, name: &str) -> Result<NewTrack, Error> {
        self.add_track(ask("newtrack", [("name", name.into())]))
    }

    fn add_track(&mut self, request: Map<String, Value>) -> Result<NewTrack, Error> {
        let answer = self.change(request)?;
        let index = index(answer.get("index")).ok_or(Error::Unexpected)?;
        let id = answer.get("id").and_then(Value::as_str).filter(|id| !id.is_empty()).map(str::to_owned);
        Ok(NewTrack { index, name: text(&answer, "name"), id })
    }

    /// [`Link::new_track`], noted as the app's audition track so [`Link::delete_audition_track`] may take it out again.
    /// Extension 3.20 and later.
    pub fn new_audition_track(&mut self, name: &str) -> Result<NewTrack, Error> {
        self.add_track(ask("newtrack", [("name", name.into()), ("audition", true.into())]))
    }

    /// Stops and takes out the track [`Link::new_audition_track`] made, only while it still has the name it came in
    /// with; with `stop_bitwig`, Bitwig stops playing too. Whether it was taken out: false when it was renamed (so it's
    /// yours now) or is gone already. Extension 3.20 and later.
    pub fn delete_audition_track(&mut self, track_id: &str, stop_bitwig: bool) -> Result<bool, Error> {
        let answer =
            self.change(ask("deletetrack", [("trackId", track_id.into()), ("transport", stop_bitwig.into())]))?;
        answer.get("deleted").and_then(Value::as_bool).ok_or(Error::Unexpected)
    }

    /// Stops the launcher clips on the track Bitwig keeps under `track_id` and puts it back on the arranger
    /// (`stoptracks`); with `stop_bitwig`, Bitwig stops playing too (extension 3.20).
    pub fn stop_track(&mut self, track_id: &str, stop_bitwig: bool) -> Result<(), Error> {
        let fields = [("trackIds", json!([track_id])), ("transport", stop_bitwig.into())];
        self.change(ask("stoptracks", fields)).map(|_| ())
    }

    /// Adds a file to the end of a track's chain (by its place in the project listing), as if it were dropped there:
    /// a sample loads into a new Sampler, a preset loads its device. `path` is the file's full path, as this computer
    /// writes it.
    pub fn insert_file(&mut self, track: usize, path: &str) -> Result<Inserted, Error> {
        let answer =
            self.change(ask("insert", [("track", track.into()), ("kind", "file".into()), ("path", path.into())]))?;
        inserted(&answer)
    }

    /// Puts a file (a MIDI file, or audio) in the first empty clip launcher slot of a track (by its place in the project
    /// listing), as if it were dropped there, and gives back the slot's place. `path` is the file's full path, as this
    /// computer writes it.
    pub fn add_clip(&mut self, track: usize, path: &str) -> Result<usize, Error> {
        let answer = self.change(ask("clip", [("track", track.into()), ("path", path.into())]))?;
        index(answer.get("slot")).ok_or(Error::Unexpected)
    }

    /// The track selected in Bitwig, or none when no track is (or it's past the listing the link reaches). Only reads,
    /// so it needs no changes allowed. Extension 3.19 and later.
    pub fn selected_track(&mut self) -> Result<Option<SelectedTrack>, Error> {
        let answer = self.request(ask("selectedtrack", []), self.timeout)?;
        if !answer.is_object() {
            return Err(Error::Unexpected);
        }
        let Some(index) = index(answer.get("index")) else {
            return Ok(None);
        };
        let id = answer.get("id").and_then(Value::as_str).filter(|id| !id.is_empty()).map(str::to_owned);
        Ok(Some(SelectedTrack { index, id, name: text(&answer, "name"), kind: text(&answer, "type") }))
    }

    /// Whether Bitwig is playing (`transport`, extension 3.16). Only reads.
    pub fn is_playing(&mut self) -> Result<bool, Error> {
        let answer = self.request(ask("transport", []), self.timeout)?;
        answer.get("playing").and_then(Value::as_bool).ok_or(Error::Unexpected)
    }

    /// Bitwig's tempo now, in BPM (`transport`, extension 3.16); none when the link doesn't say. Only reads.
    pub fn tempo(&mut self) -> Result<Option<f64>, Error> {
        let answer = self.request(ask("transport", []), self.timeout)?;
        Ok(answer.get("tempo").and_then(Value::as_f64).filter(|bpm| bpm.is_finite() && *bpm > 0.0))
    }

    /// Puts a MIDI file in the first empty clip launcher slot of the track Bitwig keeps under `track_id`, not
    /// launched, for [`Link::record_clip`] to record: noted as the app's, so [`Link::delete_audition_clip`] may take
    /// it out again. Gives back the slot. Extension 3.19 and later.
    pub fn add_clip_to_record(&mut self, track_id: &str, path: &str) -> Result<usize, Error> {
        let fields = [("trackId", track_id.into()), ("path", path.into()), ("audition", true.into())];
        let answer = self.change(ask("clip", fields))?;
        index(answer.get("slot")).ok_or(Error::Unexpected)
    }

    /// Records the launcher clip in `slot` of the track Bitwig keeps under `track_id` into the arranger, `at` beats
    /// from its start, for `length` beats (both whole bars), with the track armed, and takes the launcher clip out
    /// once it's recorded (the link's `record` job, extension 3.9). Bitwig has to be stopped. The job sets Bitwig up
    /// so only the clip records and nothing is heard (the master muted, no metronome, loop, punch or pre-roll, the
    /// other tracks disarmed, the play start at `at`), plays the clip through once, and puts everything back. It
    /// takes as long as the clip, so the answer is waited for up to `wait`.
    pub fn record_clip(
        &mut self,
        track_id: &str,
        slot: usize,
        at: f64,
        length: f64,
        wait: Duration,
    ) -> Result<Recorded, Error> {
        if self.hello.link < LINK_VERSION {
            return Err(Error::OldLink);
        }
        let fields = [
            ("trackId", track_id.into()),
            ("slot", slot.into()),
            ("at", at.into()),
            ("length", length.into()),
            // Armed is the way that stopped on the bar line in every live run (Bitwig 6.1.5, October 2026).
            ("arm", true.into()),
            ("deleteLauncherClip", true.into()),
            // Playing it back to hear it there would take as long again.
            ("replay", false.into()),
        ];
        let answer = self.request(ask("record", fields), wait)?;
        Recorded::from_json(&answer).ok_or(Error::Unexpected)
    }

    /// Puts a MIDI file in the first empty clip launcher slot of the track Bitwig keeps under `track_id` and launches
    /// it (which starts Bitwig playing, in step with the song), noted as the app's audition clip so
    /// [`Link::delete_audition_clip`] may take it out again. Gives back the slot. Extension 3.19 and later.
    ///
    /// With `song_loop` (where the song's bars start and how long they are, in beats; extension 3.22): Bitwig stops
    /// if it's playing, its arranger loops those bars, and the clip starts from its own start with the first of
    /// them. The link notes the loop and play start it found, and [`Link::delete_audition_clip`] puts them back.
    pub fn add_audition_clip(
        &mut self,
        track_id: &str,
        path: &str,
        song_loop: Option<(f64, f64)>,
    ) -> Result<usize, Error> {
        let fields =
            [("trackId", track_id.into()), ("path", path.into()), ("launch", true.into()), ("audition", true.into())];
        let mut request = ask("clip", fields);
        if let Some((start, length)) = song_loop {
            request.insert("loop".into(), json!({ "start": start, "length": length }));
        }
        let answer = self.change(request)?;
        index(answer.get("slot")).ok_or(Error::Unexpected)
    }

    /// Stops the track's clips and takes out the audition clip in `slot` ([`Link::add_audition_clip`]), only while it
    /// still has the name it came in with; with `stop_bitwig`, Bitwig stops playing too. A clip that came with the
    /// song's bars looping has the arranger's loop and the play start put back as the link found them, renamed or
    /// not ([`TakenOut`] says how both went).
    pub fn delete_audition_clip(&mut self, track_id: &str, slot: usize, stop_bitwig: bool) -> Result<TakenOut, Error> {
        let fields = [
            ("trackId", track_id.into()),
            ("slot", slot.into()),
            ("stop", true.into()),
            ("transport", stop_bitwig.into()),
        ];
        let answer = self.change(ask("deleteclip", fields))?;
        let deleted = answer.get("deleted").and_then(Value::as_bool).ok_or(Error::Unexpected)?;
        Ok(TakenOut { deleted, loop_back: answer.get("loopBack").and_then(Value::as_bool) })
    }

    /// The fields that find `track` for `cmd`: its ID when Bitwig said one and the link takes it there (`clip` from
    /// extension 3.9, `insert` and `pads` from 3.21), so the change lands on it however the tracks move meanwhile;
    /// else its place in the project listing.
    fn track_fields(&self, track: &NewTrack, cmd: &str) -> (&'static str, Value) {
        let takes_id = self.hello.runtime == "java" && (cmd == "clip" || self.hello.at_least(TRACK_IDS_EVERYWHERE));
        match &track.id {
            Some(id) if takes_id => ("trackId", id.as_str().into()),
            _ => ("track", track.index.into()),
        }
    }

    /// [`Link::insert_file`] on a track the app just made, found by its ID where the link can ([`NewTrack`]).
    pub fn insert_file_on(&mut self, track: &NewTrack, path: &str) -> Result<Inserted, Error> {
        let at = self.track_fields(track, "insert");
        inserted(&self.change(ask("insert", [at, ("kind", "file".into()), ("path", path.into())]))?)
    }

    /// [`Link::insert_device`] on a track the app just made, found by its ID where the link can.
    pub fn insert_device_on(&mut self, track: &NewTrack, device: DeviceId) -> Result<Inserted, Error> {
        let at = self.track_fields(track, "insert");
        inserted(&self.change(ask("insert", [at, ("kind", "bitwig".into()), ("device", device.to_string().into())]))?)
    }

    /// [`Link::add_clip`] on a track the app just made, found by its ID where the link can.
    pub fn add_clip_on(&mut self, track: &NewTrack, path: &str) -> Result<usize, Error> {
        let at = self.track_fields(track, "clip");
        let answer = self.change(ask("clip", [at, ("path", path.into())]))?;
        index(answer.get("slot")).ok_or(Error::Unexpected)
    }

    /// Adds one of Bitwig's own devices to the end of a track's chain, by its ID.
    pub fn insert_device(&mut self, track: usize, device: DeviceId) -> Result<Inserted, Error> {
        let fields = [("track", track.into()), ("kind", "bitwig".into()), ("device", device.to_string().into())];
        let answer = self.change(ask("insert", fields))?;
        inserted(&answer)
    }

    /// Puts samples on a Drum Machine's pads, each as if dropped on its pad (so each gets a Sampler), leaving pads
    /// that already have something on them alone. A pad holding only an empty Sampler (as a Drum Machine preset can
    /// put on every pad) counts as free. The Drum Machine is `drum_machine` (a track's place in the project listing,
    /// and the device's place in its chain), which this selects, or else the device selected in Bitwig.
    pub fn fill_pads(&mut self, pads: &[Pad], drum_machine: Option<(usize, usize)>) -> Result<PadsFilled, Error> {
        let on = drum_machine.map(|(track, device)| (("track", Value::from(track)), device));
        self.fill(pads, on)
    }

    /// [`Link::fill_pads`] on the Drum Machine at `device` on a track the app just made, found by its ID where the link
    /// can.
    pub fn fill_pads_on(&mut self, pads: &[Pad], track: &NewTrack, device: usize) -> Result<PadsFilled, Error> {
        let at = self.track_fields(track, "pads");
        self.fill(pads, Some((at, device)))
    }

    fn fill(&mut self, pads: &[Pad], on: Option<((&'static str, Value), usize)>) -> Result<PadsFilled, Error> {
        let list: Vec<Value> = pads
            .iter()
            .map(|p| {
                let name = if p.name.is_empty() { Value::Null } else { Value::from(p.name.as_str()) };
                json!({ "note": p.note, "path": p.path, "name": name })
            })
            .collect();
        let mut request = ask("pads", [("pads", list.into())]);
        if let Some(((key, track), device)) = on {
            request.insert(key.into(), track);
            request.insert("device".into(), device.into());
        }
        let answer = self.change(request)?;
        if !answer.is_object() {
            return Err(Error::Unexpected);
        }
        let taken = answer
            .get("taken")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(|pad| {
                        Some(PadContents {
                            note: note(pad.get("note"))?,
                            name: text(pad, "name"),
                            devices: whole(pad.get("devices")).unwrap_or(0),
                            first: text(pad, "first"),
                            sample: text(pad, "sample"),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(PadsFilled {
            filled: notes(&answer, "filled"),
            replaced: notes(&answer, "replaced"),
            skipped: notes(&answer, "skipped"),
            failed: notes(&answer, "failed"),
            taken,
        })
    }

    /// A request that changes something: the link waits for Bitwig to make it. Refused here, before anything is sent,
    /// when the link is too old to make changes.
    fn change(&mut self, request: Map<String, Value>) -> Result<Value, Error> {
        if self.hello.link < LINK_VERSION {
            return Err(Error::OldLink);
        }
        self.request(request, self.change_timeout)
    }

    /// Sends `request` (with its `cmd`) and waits up to `timeout` for its answer.
    fn request(&mut self, mut request: Map<String, Value>, timeout: Duration) -> Result<Value, Error> {
        if !self.open {
            return Err(Error::NotConnected);
        }
        let changes_project = request.get("cmd").and_then(Value::as_str).is_some_and(|c| PROJECT_COMMANDS.contains(&c));
        if changes_project && let Some(project) = &self.only_in {
            request.insert("onlyIn".into(), project.as_str().into());
        }
        self.last_id += 1;
        let id = self.last_id;
        request.insert("id".into(), id.into());
        self.send(ascii_json(&Value::Object(request)).as_bytes())?;

        let deadline = Instant::now() + timeout.min(LONGEST_WAIT);
        loop {
            let message = match self.next_message(deadline) {
                Ok(message) => message,
                // See the type's notes: a read that ran out of time leaves the connection unusable on Windows.
                Err(Failure::TimedOut) => {
                    self.close();
                    return Err(Error::TimedOut);
                }
                Err(Failure::Closed) => {
                    self.close();
                    return Err(Error::Closed);
                }
                Err(Failure::Garbled(why) | Failure::Broken(why)) => {
                    self.close();
                    return Err(Error::Lost(why));
                }
            };
            // An event (a greeting), or the answer to a request that timed out before.
            if message.get("event").is_some_and(Value::is_string) || message_id(&message) != Some(id) {
                continue;
            }
            return if message.get("ok").and_then(Value::as_bool) == Some(true) {
                Ok(message.get("result").cloned().unwrap_or(Value::Null))
            } else {
                let words = message.get("error").and_then(Value::as_str).filter(|e| !e.is_empty());
                Err(Error::Bitwig(words.unwrap_or("Bitwig said no.").to_owned()))
            };
        }
    }

    /// Sends one message, with its length in front, as Bitwig's remote connections take them.
    fn send(&mut self, body: &[u8]) -> Result<(), Error> {
        let length = u32::try_from(body.len()).map_err(|_| Error::Lost("the request is too big to send".into()))?;
        let mut frame = Vec::with_capacity(4 + body.len());
        frame.extend_from_slice(&length.to_be_bytes());
        frame.extend_from_slice(body);
        // A link that stopped reading mustn't hold the app up for ever.
        let written = self
            .stream
            .set_write_timeout(Some(self.timeout.clamp(Duration::from_millis(1), LONGEST_WAIT)))
            .and_then(|()| self.stream.write_all(&frame))
            .and_then(|()| self.stream.flush());
        written.map_err(|e| {
            self.close();
            Error::Lost(e.to_string())
        })
    }

    /// Closes the connection, which also tells the extension to leave out any change of this connection's still
    /// waiting behind others (3.18).
    fn close(&mut self) {
        self.open = false;
        let _ = self.stream.shutdown(Shutdown::Both);
    }

    /// The next whole message from Bitwig, waiting until `deadline` for it. Messages that aren't JSON objects are
    /// passed over, as the C# app did: the request they answer then runs out of time.
    fn next_message(&mut self, deadline: Instant) -> Result<Value, Failure> {
        loop {
            while let Some(bytes) = take_message(&mut self.pending, &mut self.scanned)? {
                if let Ok(message @ Value::Object(_)) = serde_json::from_slice::<Value>(&bytes) {
                    return Ok(message);
                }
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(Failure::TimedOut);
            }
            self.stream.set_read_timeout(Some(left)).map_err(|e| Failure::Broken(e.to_string()))?;
            let mut chunk = [0u8; 16 * 1024];
            match self.stream.read(&mut chunk) {
                Ok(0) => return Err(Failure::Closed),
                Ok(n) => self.pending.extend_from_slice(chunk.get(..n).unwrap_or_default()),
                // Out of time for this read (Windows says TimedOut, others WouldBlock): the clock decides above.
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted
                    ) => {}
                Err(e) => return Err(Failure::Broken(e.to_string())),
            }
        }
    }
}

/// Takes the first whole message off what's come in, if there is one. Bitwig puts a 4-byte big-endian length in front
/// of each; a message that starts straight away with `{` came without it (as the script once sent them) and ends at
/// its newline. Blank space between messages is passed over: as a length, it would be far over the limit anyway.
/// `scanned` is how far such a message has been searched for its newline already, so a long one that comes in many
/// reads is searched once.
pub(super) fn take_message(pending: &mut Vec<u8>, scanned: &mut usize) -> Result<Option<Vec<u8>>, Failure> {
    let blank = pending.iter().take_while(|b| matches!(b, b' ' | b'\t' | b'\r' | b'\n')).count();
    if blank > 0 {
        pending.drain(..blank);
        *scanned = 0;
    }
    let Some(&first) = pending.first() else { return Ok(None) };
    if first == b'{' {
        let from = (*scanned).min(pending.len());
        return match pending.get(from..).and_then(|rest| rest.iter().position(|&b| b == b'\n')) {
            Some(at) => {
                *scanned = 0;
                let message = pending.drain(..=from + at).collect();
                Ok(Some(message))
            }
            None if pending.len() > MAX_MESSAGE => {
                Err(Failure::Garbled("Bitwig sent a message that never ends".into()))
            }
            None => {
                *scanned = pending.len();
                Ok(None)
            }
        };
    }
    let Some(&[a, b, c, d]) = pending.first_chunk::<4>() else { return Ok(None) };
    let length = u32::from_be_bytes([a, b, c, d]) as usize;
    if length > MAX_MESSAGE {
        return Err(Failure::Garbled(format!("Bitwig sent a message of {length} bytes")));
    }
    if pending.len() < 4 + length {
        return Ok(None);
    }
    let message = pending.drain(..4 + length).skip(4).collect();
    Ok(Some(message))
}

/// A request: its command, and the fields that go with it.
fn ask<const N: usize>(cmd: &str, fields: [(&str, Value); N]) -> Map<String, Value> {
    let mut request: Map<String, Value> = fields.into_iter().map(|(field, value)| (field.to_owned(), value)).collect();
    request.insert("cmd".into(), cmd.into());
    request
}

/// A request as JSON in plain ASCII, anything else as `\u` escapes, as the C# app sent them: the extension and the
/// script read UTF-8 too, but this way no copy of either can read a name or path wrongly. Outside ASCII a character
/// can only be inside a JSON string, where an escape means the same. (The extension writes its answers the same way.)
pub(super) fn ascii_json(value: &Value) -> String {
    let json = value.to_string();
    let mut out = String::with_capacity(json.len());
    for c in json.chars() {
        if c.is_ascii() && c != '\u{7f}' {
            out.push(c);
        } else {
            let mut units = [0u16; 2];
            for unit in c.encode_utf16(&mut units) {
                let _ = write!(out, "\\u{unit:04x}");
            }
        }
    }
    out
}

/// The `id` an answer carries, if it's a whole number.
fn message_id(message: &Value) -> Option<u64> {
    whole(message.get("id")).and_then(|id| u64::try_from(id).ok())
}

/// What an `insert` with a track answered.
fn inserted(answer: &Value) -> Result<Inserted, Error> {
    if !answer.is_object() {
        return Err(Error::Unexpected);
    }
    let device = answer.get("device").filter(|d| d.is_object()).map(|d| Device {
        name: text(d, "name"),
        preset: text(d, "preset"),
        kind: text(d, "kind"),
        plugin: flag(d, "plugin"),
        enabled: d.get("enabled").and_then(Value::as_bool).unwrap_or(true),
    });
    Ok(Inserted { added: flag(answer, "added"), index: index(answer.get("index")), device })
}

/// A text field, or "" when it's missing or something else.
fn text(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or_default().to_owned()
}

/// A true-or-false field, false when it's missing or something else.
fn flag(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// A whole number, also when the link writes it as a number with a point (JavaScript has only one kind).
fn whole(v: Option<&Value>) -> Option<i64> {
    let v = v?;
    v.as_i64().or_else(|| v.as_f64().filter(|f| f.fract() == 0.0 && f.abs() < 9.0e15).map(|f| f as i64))
}

/// A place in a list (0 or more).
fn index(v: Option<&Value>) -> Option<usize> {
    whole(v).and_then(|i| usize::try_from(i).ok())
}

/// A MIDI note, 0 to 127.
fn note(v: Option<&Value>) -> Option<u8> {
    whole(v).and_then(|n| u8::try_from(n).ok()).filter(|&n| n <= 127)
}

/// A list of MIDI notes; anything in it that isn't one is left out.
fn notes(v: &Value, key: &str) -> Vec<u8> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(|n| note(Some(n))).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn framed(json: &str) -> Vec<u8> {
        let mut out = (json.len() as u32).to_be_bytes().to_vec();
        out.extend_from_slice(json.as_bytes());
        out
    }

    #[test]
    fn messages_are_taken_whole_however_they_arrive() {
        let mut stream = framed("{\"id\":1}\n");
        stream.extend_from_slice(b"{\"id\":2}\n");
        stream.extend(framed("{\"id\":3}"));
        // A byte at a time: nothing until a message is whole.
        let (mut pending, mut scanned) = (Vec::new(), 0);
        let mut got = Vec::new();
        for &b in &stream {
            pending.push(b);
            while let Some(m) = take_message(&mut pending, &mut scanned).ok().flatten() {
                got.push(String::from_utf8(m).unwrap());
            }
        }
        assert_eq!(got, ["{\"id\":1}\n", "{\"id\":2}\n", "{\"id\":3}"]);
        assert!(pending.is_empty());

        // All at once.
        let (mut pending, mut scanned) = (stream.clone(), 0);
        let mut count = 0;
        while take_message(&mut pending, &mut scanned).ok().flatten().is_some() {
            count += 1;
        }
        assert_eq!(count, 3);

        // One without its length, in pieces, then one with: each piece is searched once, and the next message starts
        // its search afresh.
        let (mut pending, mut scanned) = (b"{\"long\":\"".to_vec(), 0);
        assert!(matches!(take_message(&mut pending, &mut scanned), Ok(None)));
        assert_eq!(scanned, pending.len());
        pending.extend_from_slice(b"abc\"}\n");
        pending.extend(framed("{\"id\":4}"));
        assert_eq!(take_message(&mut pending, &mut scanned).ok().flatten().unwrap(), b"{\"long\":\"abc\"}\n");
        assert_eq!(scanned, 0);
        assert_eq!(take_message(&mut pending, &mut scanned).ok().flatten().unwrap(), b"{\"id\":4}");
    }

    #[test]
    fn a_length_past_the_limit_is_a_broken_stream() {
        let mut pending = b"HTTP/1.1 400 Bad Request\r\n".to_vec();
        assert!(matches!(take_message(&mut pending, &mut 0), Err(Failure::Garbled(_))));
        let mut pending = ((64u32 << 20) + 1).to_be_bytes().to_vec();
        assert!(matches!(take_message(&mut pending, &mut 0), Err(Failure::Garbled(_))));
        // The longest allowed is only waited for.
        let mut pending = (64u32 << 20).to_be_bytes().to_vec();
        assert!(matches!(take_message(&mut pending, &mut 0), Ok(None)));
    }

    #[test]
    fn requests_go_as_plain_ascii() {
        let json = ascii_json(&json!({ "path": "C:\\Samples\\Caf\u{e9} \u{1f941}.wav", "del": "\u{7f}" }));
        assert!(json.is_ascii());
        assert!(json.contains("Caf\\u00e9 \\ud83e\\udd41.wav"), "{json}");
        // And reads back as it was.
        let back: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(back["path"], "C:\\Samples\\Caf\u{e9} \u{1f941}.wav");
        assert_eq!(back["del"], "\u{7f}");
    }

    #[test]
    fn numbers_read_whether_written_whole_or_with_a_point() {
        assert_eq!(whole(Some(&json!(3))), Some(3));
        assert_eq!(whole(Some(&json!(3.0))), Some(3));
        assert_eq!(whole(Some(&json!(3.5))), None);
        assert_eq!(whole(Some(&json!("3"))), None);
        assert_eq!(index(Some(&json!(-1))), None);
        assert_eq!(note(Some(&json!(128))), None);
        assert_eq!(notes(&json!({ "filled": [36, 38.0, "x", 300, 42] }), "filled"), [36, 38, 42]);
    }

    #[test]
    fn reads_the_greeting_tolerantly() {
        let hello = Hello::from_json(&json!({
            "link": 3, "product": "Bitwig Studio", "version": "6.1.5", "api": 23, "platform": "WINDOWS", "port": 19791,
            "project": "Night Drive", "modified": true, "engine": true, "changes": true, "onlyIn": true,
            "runtime": "java", "extension": "3.18", "loopback": false, "something": "newer"
        }));
        assert_eq!((hello.link, hello.project.as_str(), hello.changes, hello.only_in), (3, "Night Drive", true, true));
        assert_eq!(hello.loopback, Some(false));
        assert_eq!(hello.answering(), "the Music Organiser extension 3.18");
        assert_eq!(hello.can_send(), Ok(()));

        // The script says less; a greeting with nulls reads as nothing.
        let script = Hello::from_json(&json!({ "link": 2, "product": null, "project": "X", "changes": false }));
        assert_eq!((script.product.as_str(), script.loopback), ("", None));
        assert_eq!(script.answering(), "the Music Organiser script");
        assert_eq!(script.can_send(), Err(Error::ChangesOff));
        assert_eq!(Hello::from_json(&json!({ "link": 1, "changes": true })).can_send(), Err(Error::OldLink));

        // Part of the extension didn't start: that's what's said, not that changes are off.
        let broken =
            Hello::from_json(&json!({ "link": 3, "runtime": "java", "error": "The Bitwig extension failed: x." }));
        assert_eq!(broken.can_send(), Err(Error::NotStarted("The Bitwig extension failed: x.".into())));
    }
}
