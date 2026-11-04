// Runs the Bitwig controller script in a stand-in Bitwig (fake-bitwig.js) and talks to it the way
// the app does.  npm test
"use strict";

const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const acorn = require("acorn");
const { startBitwig, SCRIPT, SAMPLER, NOTE_TRANSPOSE } = require("./fake-bitwig");

const CHANGES_OFF = /Changes are turned off/;

const song = {
  project: "Night Drive",
  tempo: 124,
  timeSignature: "7/8",
  selectedTrack: "Bass",
  tracks: [
    { name: "Drums", type: "Group", group: true, devices: [] },
    {
      name: "Bass", type: "Instrument", colour: [1, 0.5, 0], volume: [0.6, "-6.0 dB"],
      devices: [
        { name: "Serum", preset: "Reese", plugin: true, kind: "note_to_audio" },
        { name: "EQ+", plugin: false, enabled: false },
      ],
    },
    { name: "Master", type: "Master", devices: [{ name: "Peak Limiter" }] },
  ],
  actions: [
    { id: "Save", name: "Save", category: "File" },
    { id: "toggle_metronome", name: "Toggle Metronome", category: "Transport" },
  ],
};

test("it's plain ES5, which every Bitwig script engine runs", () => {
  acorn.parse(fs.readFileSync(SCRIPT, "utf8"), { ecmaVersion: 5 });
});

test("it's plain ASCII, so it reads the same in any encoding", () => {
  assert.doesNotMatch(fs.readFileSync(SCRIPT, "latin1"), /[^\x00-\x7f]/);
});

test("says hello when the app connects", () => {
  const bitwig = startBitwig(song);
  assert.equal(bitwig.api, 18);
  const [vendor, model, , id] = bitwig.defined;
  assert.deepEqual([vendor, model], ["Music Organiser", "DAW link"]);
  assert.match(id, /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);

  const app = bitwig.connect();
  const [hello] = app.take();
  assert.equal(hello.event, "hello");
  assert.deepEqual(hello.result, {
    link: 2, product: "Bitwig Studio", version: "6.1.2", api: 25, platform: "WINDOWS", port: 19791,
    project: "Night Drive", modified: false, engine: true, changes: false,
    onlyIn: true,   // it honours a request's onlyIn
  });
  assert.deepEqual(app.ask({ id: 1, cmd: "hello" }), { id: 1, ok: true, result: hello.result });
  assert.deepEqual(bitwig.popups, []);
});

test("warns in Bitwig when its port is taken", () => {
  const bitwig = startBitwig({ portInUse: 50123 });
  assert.match(bitwig.popups[0], /port 19791 is taken/);
  // Most often by the link itself, loaded twice.
  assert.match(bitwig.popups[0], /listed twice in Bitwig's Settings > Controllers, remove one/);
});

test("doesn't say the port is taken while Bitwig hasn't said which port it gave", () => {
  // Bitwig 6.1.3 says -1 straight after making the connection, and the app connects on 19791 all the same.
  const bitwig = startBitwig({ ...song, portUnknown: 1000 });
  bitwig.clock.advance(2000);
  assert.deepEqual(bitwig.popups, []);
  const app = bitwig.connect();
  assert.equal(app.take()[0].result.port, 19791);
  assert.deepEqual(bitwig.popups, []);
});

test("says the port is taken once Bitwig says it gave another", () => {
  const bitwig = startBitwig({ ...song, portUnknown: 1, portInUse: 50123 });
  assert.deepEqual(bitwig.popups, []);
  bitwig.clock.advance(1000);
  assert.equal(bitwig.popups.length, 1);
  assert.match(bitwig.popups[0], /port 19791 is taken/);
  bitwig.connect().take();
  assert.equal(bitwig.popups.length, 1);
});

test("lists the project's tracks and their devices", () => {
  const bitwig = startBitwig(song);
  const app = bitwig.connect();
  app.take();
  const reply = app.ask({ id: "p", cmd: "project" });
  assert.equal(reply.ok, true);
  const p = reply.result;
  assert.deepEqual([p.name, p.tempo, p.timeSignature, p.trackCount], ["Night Drive", 124, "7/8", 3]);
  assert.deepEqual(p.tracks.map((t) => t.name), ["Drums", "Bass", "Master"]);
  const bass = p.tracks[1];
  assert.deepEqual(
    { index: bass.index, type: bass.type, group: bass.group, color: bass.color, volume: bass.volume, volumeValue: bass.volumeValue },
    { index: 1, type: "Instrument", group: false, color: "#ff8000", volume: "-6.0 dB", volumeValue: 0.6 });
  assert.deepEqual(bass.devices, [
    { index: 0, name: "Serum", preset: "Reese", kind: "note_to_audio", plugin: true, enabled: true },
    { index: 1, name: "EQ+", preset: "", kind: "audio_to_audio", plugin: false, enabled: false },
  ]);
  assert.equal(bass.deviceCount, 2);
  assert.equal(p.tracks[0].group, true);
});

test("waits for the selected device's parameter texts", () => {
  const bitwig = startBitwig(song);
  bitwig.cursorDevice.show({ name: "Serum", preset: "Reese", plugin: true }, [
    { id: "CONTENTS/PID3e8", name: "Cutoff", value: 0.5, display: "440 Hz" },
    { id: "CONTENTS/PID3e9", name: "Mix", value: NaN, display: "50 %" },
  ]);
  const app = bitwig.connect();
  app.take();

  app.send({ id: 7, cmd: "device" });
  assert.deepEqual(app.take(), [], "answers only once the texts are in");
  // (Array.from: the script's arrays come from its own JavaScript realm.)
  assert.deepEqual(Array.from(bitwig.cursorDevice.observed), ["CONTENTS/PID3e8", "CONTENTS/PID3e9"]);
  bitwig.clock.advance(100);
  const [reply] = app.take();
  assert.equal(reply.id, 7);
  assert.equal(reply.result.track, "Bass");
  assert.equal(reply.result.device.name, "Serum");
  assert.deepEqual(reply.result.params, [
    { id: "CONTENTS/PID3e8", name: "Cutoff", value: 0.5, display: "440 Hz" },
    { id: "CONTENTS/PID3e9", name: "Mix", value: null, display: "50 %" },
  ]);
  assert.equal(bitwig.cursorDevice.observed, null, "stops watching the texts afterwards");
});

test("gives up on parameter texts that never come", () => {
  const bitwig = startBitwig(song);
  bitwig.cursorDevice.show({ name: "Odd Plugin", plugin: true }, [
    { id: "a", name: "Talks", value: 1, display: "on" },
    { id: "b", name: "Silent", value: 0 },
  ]);
  const app = bitwig.connect();
  app.take();
  app.send({ id: 1, cmd: "device" });
  app.send({ id: 2, cmd: "device" });   // asked twice while waiting: both get the answer
  bitwig.clock.advance(1900);
  assert.deepEqual(app.take(), []);
  bitwig.clock.advance(200);
  const replies = app.take();
  assert.deepEqual(replies.map((r) => r.id), [1, 2]);
  assert.deepEqual(replies[0].result.params.map((p) => p.display), ["on", null]);
});

test("says when no device is selected", () => {
  const bitwig = startBitwig(song);
  const app = bitwig.connect();
  app.take();
  assert.deepEqual(app.ask({ id: 3, cmd: "device" }).result, { track: "Bass", device: null, params: [], drumPads: false });
});

test("forgets parameters of the previous device", () => {
  const bitwig = startBitwig(song);
  bitwig.cursorDevice.show({ name: "One" }, [{ id: "x", name: "Old", value: 0.1, display: "1" }]);
  bitwig.cursorDevice.show({ name: "Two" }, [{ id: "y", name: "New", value: 0.2, display: "2" }]);
  const app = bitwig.connect();
  app.take();
  app.send({ id: 1, cmd: "device" });
  bitwig.clock.advance(100);
  assert.deepEqual(app.take()[0].result.params, [{ id: "y", name: "New", value: 0.2, display: "2" }]);
});

test("changes nothing until changes are allowed in Bitwig", () => {
  const bitwig = startBitwig(song);
  bitwig.cursorDevice.show({ name: "Serum" }, [{ id: "p", name: "Cutoff", value: 0 }]);
  const app = bitwig.connect();
  app.take();
  for (const request of [
    { id: 1, cmd: "set", param: "p", value: 0.5 },
    { id: 2, cmd: "insert", kind: "vst3", plugin: "x" },
    { id: 3, cmd: "action", action: "Save" },
    { id: 4, cmd: "newtrack", name: "Lead" },
    { id: 5, cmd: "clip", track: 1, path: "C:\\Lead.mid" },
    { id: 6, cmd: "pads", pads: [{ note: 36, path: "C:\\Kick.wav" }] },
    { id: 7, cmd: "transpose", track: 1, device: 0, semitones: 2 },
  ]) {
    const reply = app.ask(request);
    assert.equal(reply.ok, false);
    assert.match(reply.error, CHANGES_OFF);
  }
  assert.deepEqual(bitwig.calls, []);
  assert.deepEqual(bitwig.cursorDevice.setCalls, []);
});

test("sets the selected device's parameters", () => {
  const bitwig = startBitwig(song);
  bitwig.allowChanges(true);
  bitwig.cursorDevice.show({ name: "Serum" }, [{ id: "CONTENTS/PID3e8", name: "Cutoff", value: 0 }]);
  const app = bitwig.connect();
  assert.equal(app.take()[0].result.changes, true);

  assert.equal(app.ask({ id: 1, cmd: "set", param: "CONTENTS/PID3e8", value: 0.25 }).ok, true);
  assert.deepEqual(bitwig.cursorDevice.setCalls, [["CONTENTS/PID3e8", 250000, 1000001]]);
  assert.match(app.ask({ id: 2, cmd: "set", param: "nope", value: 0.25 }).error, /no parameter nope/);
  assert.match(app.ask({ id: 3, cmd: "set", param: "CONTENTS/PID3e8", value: 2 }).error, /from 0 to 1/);
  assert.match(app.ask({ id: 4, cmd: "set", param: "CONTENTS/PID3e8", value: "0.5" }).error, /from 0 to 1/);
});

test("inserts plugins, Bitwig devices, presets and samples", () => {
  const bitwig = startBitwig({ ...song, java: true });
  bitwig.allowChanges(true);
  const app = bitwig.connect();
  app.take();
  const ok = (request) => assert.equal(app.ask(request).ok, true, JSON.stringify(request));
  ok({ id: 1, cmd: "insert", kind: "vst3", plugin: "805ECA49563E43EA8119369F0ED6096D" });
  ok({ id: 2, cmd: "insert", kind: "clap", plugin: "org.musicorganiser.probe.effect" });
  ok({ id: 3, cmd: "insert", kind: "vst2", plugin: 1299137074 });
  ok({ id: 4, cmd: "insert", kind: "bitwig", device: "c9a8b2f5-4f50-4f5c-98f2-a7b3c1e0d1aa" });
  ok({ id: 5, cmd: "insert", kind: "file", path: "C:\\Samples\\Café\\Kick ß.wav" });
  assert.deepEqual(bitwig.calls, [
    ["insertVST3Device", "805ECA49563E43EA8119369F0ED6096D"],
    ["insertCLAPDevice", "org.musicorganiser.probe.effect"],
    ["insertVST2Device", 1299137074],
    ["insertBitwigDevice", { uuid: "c9a8b2f5-4f50-4f5c-98f2-a7b3c1e0d1aa", javaType: "java.util.UUID" }],
    ["insertFile", "C:\\Samples\\Café\\Kick ß.wav"],
  ]);
  assert.match(app.ask({ id: 6, cmd: "insert", kind: "vst2", plugin: "MoB2" }).error, /is a number/);
  assert.match(app.ask({ id: 7, cmd: "insert", kind: "aax", plugin: "x" }).error, /use vst3, clap, vst2, bitwig or file/);
  assert.match(app.ask({ id: 9, cmd: "insert", kind: "vst3" }).error, /No plugin ID given/);
  assert.match(app.ask({ id: 8, cmd: "insert", kind: "file", path: "" }).error, /No file/);
});

test("lists and runs Bitwig's actions", () => {
  const bitwig = startBitwig(song);
  const app = bitwig.connect();
  app.take();
  assert.deepEqual(app.ask({ id: 1, cmd: "actions" }).result, [
    { id: "Save", name: "Save", category: "File", menu: "Save" },
    { id: "toggle_metronome", name: "Toggle Metronome", category: "Transport", menu: "Toggle Metronome" },
  ]);
  bitwig.allowChanges(true);
  // The answer names the project Bitwig had in front as the action ran, and that it ran straight away (as the Java extension 3.7 says).
  assert.deepEqual(app.ask({ id: 2, cmd: "action", action: "Save" }), {
    id: 2,
    ok: true,
    result: { project: "Night Drive", via: "direct", ran: true },
  });
  assert.deepEqual(bitwig.calls, [["invoke", "Save"]]);
  assert.match(app.ask({ id: 3, cmd: "action", action: "Format C:" }).error, /no action Format C:/);
  bitwig.app.name.v = "My Song";
  assert.deepEqual(app.ask({ id: 4, cmd: "action", action: "toggle_metronome" }).result, { project: "My Song", via: "direct", ran: true });
});

test("runs an action straight away only, and says the other routes need the Java extension", () => {
  const bitwig = startBitwig(song);
  bitwig.allowChanges(true);
  const app = bitwig.connect();
  app.take();
  for (const via of [null, "direct"])
    assert.deepEqual(app.ask({ id: 1, cmd: "action", action: "Save", via }).result, { project: "Night Drive", via: "direct", ran: true });
  for (const via of ["flush", "midi", "button", "buttonCode", "Direct", 5])
    assert.equal(app.ask({ id: 2, cmd: "action", action: "Save", via }).error, "This needs the Java extension 3.7.");
  assert.deepEqual(bitwig.calls, [["invoke", "Save"], ["invoke", "Save"]]);
});

test("selects a track and one of its devices", () => {
  const bitwig = startBitwig(song);
  const app = bitwig.connect();
  app.take();
  assert.equal(app.ask({ id: 1, cmd: "select", track: 1, device: 1 }).ok, true);
  assert.equal(bitwig.calls[0][0], "selectChannel");
  assert.equal(bitwig.calls[0][1], bitwig.tracks.getItemAt(1));
  assert.deepEqual(bitwig.cursorDevice.selected, [bitwig.tracks.getItemAt(1).devices.getItemAt(1)]);
  assert.match(app.ask({ id: 2, cmd: "select", track: 3 }).error, /no track 3/);
  assert.match(app.ask({ id: 3, cmd: "select", track: 0.5 }).error, /no track 0.5/);
  assert.match(app.ask({ id: 4, cmd: "select", track: 1, device: 2 }).error, /has no device 2/);
});

test("answers requests it can't follow with an error", () => {
  const bitwig = startBitwig(song);
  const app = bitwig.connect();
  app.take();
  assert.deepEqual(app.ask("{not json"), { id: null, ok: false, error: "The request isn't JSON: {not json" });
  assert.deepEqual(app.ask({ id: 2, cmd: "explode" }), { id: 2, ok: false, error: "Unknown command: explode" });
  assert.equal(app.ask({ id: 3, cmd: "toString" }).error, "Unknown command: toString");
  assert.equal(app.ask({ id: 4 }).error, "Unknown command: undefined");
  assert.equal(app.ask("null").error, "Unknown command: null");
});

test("keeps text outside ASCII intact", () => {
  const name = "Café ☕ 𝄞 Ünïcödé";
  const bitwig = startBitwig({ ...song, tracks: [{ name, devices: [] }] });
  const app = bitwig.connect();
  app.take();
  assert.equal(app.ask({ id: 1, cmd: "project" }).result.tracks[0].name, name);
});

test("stops sending to an app that disconnected", () => {
  const bitwig = startBitwig(song);
  bitwig.cursorDevice.show({ name: "Serum" }, [{ id: "p", name: "Cutoff", value: 0, display: "0" }]);
  const app = bitwig.connect();
  app.take();
  app.send({ id: 1, cmd: "device" });
  app.close();
  bitwig.clock.advance(100);
  assert.deepEqual(app.take(), []);
  assert.deepEqual(bitwig.errors, []);
});

// ---- Sending things into Bitwig ----

const DRUM_MACHINE = "d7d7d7d7-0000-4000-8000-000000000001";   // the stand-in's; Bitwig's own isn't known

/** A project with effect tracks and changes allowed, ready for the app to send things into. */
function studio(extra = {}) {
  const bitwig = startBitwig({
    project: "Night Drive",
    java: true,
    drumMachine: DRUM_MACHINE,
    selectedTrack: "Bass",
    tracks: [
      { name: "Bass", type: "Instrument", devices: [{ name: "Serum", plugin: true, kind: "note_to_audio" }] },
      { name: "Reverb", type: "Effect", devices: [{ name: "Reverb" }] },
      { name: "Master", type: "Master", devices: [] },
    ],
    files: { "C:\\Presets\\Lead.bwpreset": { name: "Serum", preset: "Lead", plugin: true, kind: "note_to_audio" } },
    plugins: { "VST3:805ECA49563E43EA8119369F0ED6096D": { name: "Serum", plugin: true, kind: "note_to_audio" } },
    ...extra,
  });
  bitwig.allowChanges(true);
  const app = bitwig.connect();
  app.take();
  return { bitwig, app };
}

test("adds a named, coloured instrument track after the main tracks and selects it", () => {
  const { bitwig, app } = studio();
  const reply = app.wait({ id: 1, cmd: "newtrack", name: "Lead", color: "#ff8000" });
  assert.deepEqual(reply, { id: 1, ok: true, result: { index: 1, name: "Lead" } });
  const tracks = app.ask({ id: 2, cmd: "project" }).result.tracks;
  assert.deepEqual(tracks.map((t) => [t.name, t.type]), [["Bass", "Instrument"], ["Lead", "Instrument"], ["Reverb", "Effect"], ["Master", "Master"]]);
  assert.equal(tracks[1].color, "#ff8000");
  assert.deepEqual(bitwig.calls.filter((c) => c[0] === "selectChannel").map((c) => c[1]), [bitwig.tracks.getItemAt(1)]);
  assert.equal(app.ask({ id: 3, cmd: "device" }).result.track, "Lead");

  // Without a name, the track keeps Bitwig's.
  assert.deepEqual(app.wait({ id: 4, cmd: "newtrack" }).result, { index: 2, name: "Inst 3" });
  assert.match(app.ask({ id: 5, cmd: "newtrack", color: "orange" }).error, /#rrggbb/);
  assert.match(app.ask({ id: 6, cmd: "newtrack", name: 7 }).error, /name is text/);
});

test("closes Bitwig's browser if a new track opens it", () => {
  const { bitwig, app } = studio({ browserOnNewTrack: true });
  assert.equal(app.wait({ id: 1, cmd: "newtrack", name: "Lead" }).ok, true);
  assert.deepEqual(bitwig.calls.filter((c) => c[0] === "cancelBrowser"), [["cancelBrowser"]]);
});

test("says when Bitwig adds a track where the app can't see it", () => {
  const full = Array.from({ length: 128 }, (_, i) => ({ name: `Track ${i + 1}`, devices: [] }));
  const { app } = studio({ tracks: full });
  assert.match(app.wait({ id: 1, cmd: "newtrack", name: "Lead" }).error, /past the first 128 tracks/);
});

test("inserts onto a given track and says whether Bitwig added a device", () => {
  const { bitwig, app } = studio();
  const sample = app.wait({ id: 1, cmd: "insert", track: 0, kind: "file", path: "C:\\Samples\\Pluck.wav" });
  assert.deepEqual(sample.result, {
    added: true, index: 1,
    device: { index: 1, name: "Sampler", preset: "Pluck", kind: "note_to_audio", plugin: false, enabled: true },
  });
  const first = app.wait({ id: 2, cmd: "insert", track: 0, at: "start", kind: "bitwig", device: NOTE_TRANSPOSE });
  assert.deepEqual([first.result.added, first.result.index, first.result.device.name], [true, 0, "Note Transpose"]);
  assert.deepEqual(bitwig.state.tracks[0].devices.map((d) => d.name), ["Note Transpose", "Serum", "Sampler"]);

  const preset = app.wait({ id: 3, cmd: "insert", track: 2, kind: "file", path: "C:\\Presets\\Lead.bwpreset" });
  assert.deepEqual([preset.result.device.name, preset.result.device.preset], ["Serum", "Lead"]);

  // Bitwig ignores what it can't use, so after waiting the answer says nothing was added.
  const unknown = app.wait({ id: 4, cmd: "insert", track: 0, kind: "vst3", plugin: "00000000000000000000000000000000" });
  assert.deepEqual(unknown.result, { added: false, index: null, device: null });
  assert.match(app.ask({ id: 5, cmd: "insert", track: 9, kind: "file", path: "x.wav" }).error, /no track 9/);
  assert.match(app.ask({ id: 6, cmd: "insert", track: 0, at: "middle", kind: "file", path: "x.wav" }).error, /"start" or the "end"/);
  assert.match(app.ask({ id: 7, cmd: "insert", at: "start", kind: "file", path: "x.wav" }).error, /Say which track/);
});

test("sets the pitch of a Sampler or a Note Transpose at the start of a track", () => {
  const { bitwig, app } = studio();
  app.wait({ id: 1, cmd: "newtrack", name: "Pluck" });
  app.wait({ id: 2, cmd: "insert", track: 1, kind: "file", path: "C:\\Samples\\Pluck.wav" });
  assert.deepEqual(app.wait({ id: 3, cmd: "transpose", track: 1, device: 0, semitones: -3.5 }).result,
                   { device: "Sampler", value: -3.5, display: "-3.50 st" });
  assert.equal(bitwig.state.tracks[1].devices[0].params.TRANSPOSE, -3.5);

  app.wait({ id: 4, cmd: "insert", track: 0, at: "start", kind: "bitwig", device: NOTE_TRANSPOSE });
  assert.deepEqual(app.wait({ id: 5, cmd: "transpose", track: 0, device: 0, semitones: 7 }).result,
                   { device: "Note Transpose", value: 7, display: "+7 st" });
  // What Bitwig ends up with comes back, whatever was asked for.
  assert.equal(app.wait({ id: 6, cmd: "transpose", track: 0, device: 0, semitones: 60 }).result.value, 48);

  assert.match(app.wait({ id: 7, cmd: "transpose", track: 0, device: 1, semitones: 2 }).error, /neither a Sampler nor a Note Transpose/);
  assert.match(app.ask({ id: 8, cmd: "transpose", track: 0, device: 2, semitones: 2 }).error, /first 2 devices/);
  assert.match(app.ask({ id: 9, cmd: "transpose", track: 1, device: 0, semitones: "up" }).error, /how many semitones/);
});

test("puts a MIDI file in the track's first empty clip slot", () => {
  const { bitwig, app } = studio({ scenes: 2 });
  bitwig.state.tracks[0].slots = [{ file: "old.mid" }];
  bitwig.refresh();
  assert.deepEqual(app.wait({ id: 1, cmd: "clip", track: 0, path: "C:\\MIDI\\Bass.mid" }).result, { slot: 1 });
  // With the project's scenes full, the next clip starts a new scene.
  assert.deepEqual(app.wait({ id: 2, cmd: "clip", track: 0, path: "C:\\MIDI\\Bass 2.mid" }).result, { slot: 2 });
  assert.deepEqual(bitwig.state.tracks[0].slots.map((c) => c.file), ["old.mid", "C:\\MIDI\\Bass.mid", "C:\\MIDI\\Bass 2.mid"]);
  assert.deepEqual(bitwig.calls.filter((c) => /slot/.test(c[2] || "")).map((c) => c[2]), ["track 0 slot 1", "track 0 slot 2"]);

  assert.match(app.ask({ id: 3, cmd: "clip", track: 0, slot: 0, path: "C:\\MIDI\\x.mid" }).error, /already has a clip/);
  assert.match(app.ask({ id: 4, cmd: "clip", track: 0, slot: 8, path: "C:\\MIDI\\x.mid" }).error, /first 8 clip slots/);
  assert.match(app.wait({ id: 5, cmd: "clip", track: 0, path: "C:\\notes.txt" }).error, /didn't make a clip/);
  bitwig.state.scenes = 8;
  bitwig.state.tracks[0].slots = Array.from({ length: 8 }, () => ({ file: "x.mid" }));
  bitwig.refresh();
  assert.match(app.ask({ id: 6, cmd: "clip", track: 0, path: "C:\\MIDI\\y.mid" }).error, /clip slots are full/);
});

test("fills a new Drum Machine's empty pads, from the lowest note", () => {
  const { bitwig, app } = studio({ padScroll: 36 });
  app.wait({ id: 1, cmd: "newtrack", name: "Kit #7" });
  const machine = app.wait({ id: 2, cmd: "insert", track: 1, kind: "bitwig", device: DRUM_MACHINE });
  assert.equal(machine.result.device.name, "Drum Machine");

  const pads = [
    { note: 36, path: "C:\\Kit\\Kick.wav", name: "Kick" },
    { note: 38, path: "C:\\Kit\\Snare.wav", name: "Snare" },
    { note: 42, path: "C:\\Kit\\notes.txt", name: "Closed hat" },
  ];
  const reply = app.wait({ id: 3, cmd: "pads", track: 1, device: 0, pads });
  assert.deepEqual(reply.result, { filled: [36, 38], replaced: [], skipped: [], failed: [42], taken: [] });
  assert.equal(bitwig.pads.indication, false, "doesn't light up the pads in Bitwig");
  const filled = bitwig.state.tracks[1].devices[0].pads;
  assert.deepEqual(Object.keys(filled), ["36", "38"]);
  assert.deepEqual([filled[36].name, filled[36].devices[0].sample], ["Kick", "C:\\Kit\\Kick.wav"]);

  // A pad with something on it is left alone.
  const again = app.wait({ id: 4, cmd: "pads", track: 1, device: 0, pads: [{ note: 36, path: "C:\\Kit\\Kick 2.wav" }, { note: 37, path: "C:\\Kit\\Rim.wav" }] });
  assert.deepEqual(again.result, { filled: [37], replaced: [], skipped: [36], failed: [],
                                  taken: [{ note: 36, name: "Kick", devices: 1, first: "Sampler", sample: "Kick.wav" }] });
  assert.equal(filled[36].devices.length, 1);
});

test("puts samples in place of the empty Samplers a default Drum Machine has on its pads, keeping what's after them", () => {
  // Solomon's default Drum Machine has an empty Sampler on every pad.
  const { bitwig, app } = studio({ drumPads: [{ name: "Sampler", id: SAMPLER, kind: "note_to_audio" }, { name: "Compressor" }] });
  app.wait({ id: 1, cmd: "newtrack", name: "Kit #8" });
  app.wait({ id: 2, cmd: "insert", track: 1, kind: "bitwig", device: DRUM_MACHINE });
  const kit = [{ note: 36, path: "C:\\Kit\\Kick.wav", name: "Kick" }, { note: 38, path: "C:\\Kit\\Snare.wav" }];
  assert.deepEqual(app.wait({ id: 3, cmd: "pads", track: 1, device: 0, pads: kit }).result,
                   { filled: [36, 38], replaced: [36, 38], skipped: [], failed: [], taken: [] });
  const pads = bitwig.state.tracks[1].devices[0].pads;
  assert.deepEqual(pads[36].devices.map((d) => d.name), ["Sampler", "Compressor"]);
  assert.deepEqual([pads[36].name, pads[36].devices[0].sample], ["Kick", "C:\\Kit\\Kick.wav"]);
  assert.equal(pads[38].devices[0].sample, "C:\\Kit\\Snare.wav");
  assert.equal(pads[40].devices[0].sample, undefined);

  // A pad whose Sampler has a sample now is left alone.
  assert.deepEqual(app.wait({ id: 4, cmd: "pads", track: 1, device: 0,
                              pads: [{ note: 36, path: "C:\\Kit\\Kick 2.wav" }, { note: 40, path: "C:\\Kit\\Rim.wav" }] }).result,
                   { filled: [40], replaced: [40], skipped: [36], failed: [],
                     taken: [{ note: 36, name: "Kick", devices: 2, first: "Sampler", sample: "Kick.wav" }] });
  assert.equal(pads[36].devices[0].sample, "C:\\Kit\\Kick.wav");

  // Nor is one whose empty Sampler has an instrument after it, which plays; effects after it don't. Bitwig 6.1.5
  // calls an instrument "instrument", and (most likely) an audio effect "audio-effect".
  pads[42].devices.push({ name: "Polymer", kind: "note_to_audio" });
  pads[44].devices.push({ name: "Delay+" });
  pads[46].devices.push({ name: "EQ+", kind: "audio-effect" });
  pads[48].devices.push({ name: "Polymer", kind: "instrument" });
  bitwig.clock.advance(100);
  assert.deepEqual(app.wait({ id: 5, cmd: "pads", track: 1, device: 0,
                              pads: [{ note: 42, path: "C:\\Kit\\Hat.wav" }, { note: 44, path: "C:\\Kit\\Clap.wav" },
                                     { note: 46, path: "C:\\Kit\\Tom.wav" }, { note: 48, path: "C:\\Kit\\Ride.wav" }] }).result,
                   { filled: [44, 46], replaced: [44, 46], skipped: [42, 48], failed: [],
                     taken: [{ note: 42, name: "Sampler", devices: 3, first: "Sampler", sample: "" },
                             { note: 48, name: "Sampler", devices: 3, first: "Sampler", sample: "" }] });
  assert.deepEqual(pads[42].devices.map((d) => d.name), ["Sampler", "Compressor", "Polymer"]);
  assert.equal(pads[42].devices[0].sample, undefined);
});

test("reads and sets the knobs of a plugin it put on a track, once that's selected", () => {
  // How the app copies FL Studio's knobs across and checks they took.
  const { bitwig, app } = studio({
    plugins: {
      "VST3:805ECA49563E43EA8119369F0ED6096D": {
        name: "Serum", plugin: true, kind: "note_to_audio",
        knobs: [
          { id: "CONTENTS/PID0", name: "Master Volume", value: 0.5, display: "50%" },
          { id: "CONTENTS/PID3e8", name: "Filter Cutoff", value: 0.5, display: "50%" },
        ],
      },
    },
  });
  const track = app.wait({ id: 1, cmd: "newtrack", name: "Lead" }).result.index;
  const added = app.wait({ id: 2, cmd: "insert", track, kind: "vst3", plugin: "805ECA49563E43EA8119369F0ED6096D" }).result;
  app.wait({ id: 3, cmd: "select", track, device: added.index });
  const before = app.wait({ id: 4, cmd: "device" }).result;
  assert.deepEqual([before.track, before.device.name], ["Lead", "Serum"]);
  assert.deepEqual(before.params.map((p) => [p.name, p.value]), [["Master Volume", 0.5], ["Filter Cutoff", 0.5]]);

  assert.equal(app.ask({ id: 5, cmd: "set", param: "CONTENTS/PID3e8", value: 0.25 }).ok, true);
  bitwig.clock.advance(100);
  const after = app.wait({ id: 6, cmd: "device" }).result.params;
  assert.deepEqual(after.map((p) => [p.name, p.value, p.display]), [["Master Volume", 0.5, "50%"], ["Filter Cutoff", 0.25, "25%"]]);
});

test("fills the selected Drum Machine, and says when the selected device isn't one", () => {
  const { bitwig, app } = studio();
  app.wait({ id: 1, cmd: "insert", track: 0, at: "start", kind: "bitwig", device: DRUM_MACHINE });
  app.wait({ id: 2, cmd: "select", track: 0, device: 0 });
  assert.equal(app.wait({ id: 3, cmd: "device" }).result.drumPads, true);
  assert.deepEqual(app.wait({ id: 4, cmd: "pads", pads: [{ note: 40, path: "C:\\Kit\\Snare 2.wav" }] }).result,
                   { filled: [40], replaced: [], skipped: [], failed: [], taken: [] });
  assert.deepEqual(Object.keys(bitwig.state.tracks[0].devices[0].pads), ["40"]);

  app.wait({ id: 5, cmd: "select", track: 0, device: 1 });
  assert.match(app.wait({ id: 6, cmd: "pads", pads: [{ note: 41, path: "C:\\Kit\\Tom.wav" }] }).error, /selected device isn't a Drum Machine/);
  assert.match(app.wait({ id: 7, cmd: "pads", track: 0, device: 1, pads: [{ note: 41, path: "C:\\Kit\\Tom.wav" }] }).error,
               /Device 1 on track 0 isn't a Drum Machine/);
  for (const [pads, error] of [
    [[], /No pads given/],
    [[{ note: 128, path: "a.wav" }], /from 0 to 127/],
    [[{ note: 36, path: "a.wav" }, { note: 36, path: "b.wav" }], /Two samples for note 36/],
    [[{ note: 36 }], /No sample given/],
  ])
    assert.match(app.ask({ id: 8, cmd: "pads", pads }).error, error);
});

test("makes changes one at a time, in the order asked", () => {
  const { bitwig, app } = studio();
  app.send({ id: 1, cmd: "newtrack", name: "One" });
  app.send({ id: 2, cmd: "newtrack", name: "Two" });
  app.send({ id: 3, cmd: "project" });   // reading doesn't wait for changes
  const [project] = app.take();
  assert.equal(project.id, 3);
  bitwig.clock.advance(20000);
  const replies = app.take();
  assert.deepEqual(replies.map((r) => [r.id, r.result.index, r.result.name]), [[1, 1, "One"], [2, 2, "Two"]]);
});

// ---- Only in the project asked for ----

/** The answer to a request that names one project (onlyIn) while Bitwig has another open. */
function elsewhere(open, asked) {
  return `Bitwig has "${open}" open, not "${asked}", so the link stopped before changing anything there.`;
}

/** Each track as [name, colour], to see nothing was renamed or coloured. */
function looks(bitwig) {
  return bitwig.state.tracks.map((t) => [t.name, t.colour]);
}

test("makes a change only in the project the request names", () => {
  const { bitwig, app } = studio();
  assert.deepEqual(app.wait({ id: 1, cmd: "newtrack", name: "Lead", color: "#ff8000", onlyIn: "Night Drive" }),
                   { id: 1, ok: true, result: { index: 1, name: "Lead" } });
  assert.equal(app.ask({ id: 2, cmd: "select", track: 0, onlyIn: "Night Drive" }).ok, true);
  assert.equal(app.ask({ id: 3, cmd: "project", onlyIn: "Night Drive" }).result.tracks[1].color, "#ff8000");
  // null is the same as no onlyIn: no check.
  assert.deepEqual(app.wait({ id: 4, cmd: "clip", track: 0, path: "C:\\MIDI\\Bass.mid", onlyIn: null }).result, { slot: 0 });
  assert.equal(bitwig.state.tracks[0].slots[0].file, "C:\\MIDI\\Bass.mid");
});

test("refuses a request meant for another project, and changes nothing", () => {
  const { bitwig, app } = studio();
  for (const request of [
    { cmd: "newtrack", name: "Lead" },
    { cmd: "insert", track: 0, kind: "file", path: "C:\\Samples\\Pluck.wav" },
    { cmd: "insert", kind: "vst3", plugin: "805ECA49563E43EA8119369F0ED6096D" },
    { cmd: "clip", track: 0, path: "C:\\MIDI\\Bass.mid" },
    { cmd: "pads", track: 0, device: 0, pads: [{ note: 36, path: "C:\\Kit\\Kick.wav" }] },
    { cmd: "transpose", track: 0, device: 0, semitones: 2 },
    { cmd: "set", param: "p", value: 0.5 },
    { cmd: "action", action: "Save" },
    { cmd: "select", track: 0, device: 0 },
    { cmd: "project" },
  ]) {
    assert.deepEqual(app.wait({ id: 1, ...request, onlyIn: "MO Live Test" }),
                     { id: 1, ok: false, error: elsewhere("Night Drive", "MO Live Test") }, request.cmd);
  }
  // The name has to be the same exactly.
  assert.equal(app.ask({ id: 2, cmd: "select", track: 0, onlyIn: "night drive" }).error, elsewhere("Night Drive", "night drive"));
  // An onlyIn that isn't a project's name is refused too.
  for (const onlyIn of [5, true, {}, ["Night Drive"]])
    assert.deepEqual(app.ask({ id: 3, cmd: "newtrack", onlyIn }), { id: 3, ok: false, error: "onlyIn has to be a project's name." });
  bitwig.clock.advance(10000);
  assert.deepEqual(bitwig.calls, []);
  assert.deepEqual(bitwig.cursorDevice.setCalls, []);
  assert.deepEqual(looks(bitwig), [["Bass", undefined], ["Reverb", undefined], ["Master", undefined]]);
});

test("stops a new track part way when another project comes to the front, and the next change still runs", () => {
  const { bitwig, app } = studio({ browserOnNewTrack: true });
  // Just after Bitwig is asked for the track, someone brings their own project to the front.
  bitwig.switchDuring("newtrack", "My Song");
  assert.deepEqual(app.wait({ id: 1, cmd: "newtrack", name: "Lead", color: "#ff8000", onlyIn: "Night Drive" }),
                   { id: 1, ok: false, error: elsewhere("My Song", "Night Drive") });
  // The tracks Bitwig shows now are My Song's (the stand-in keeps one project, so the track Bitwig had just
  // added shows there too): none is renamed or coloured, the selection stays put, and Bitwig's browser is left
  // alone.
  assert.deepEqual(looks(bitwig), [["Bass", undefined], ["Inst 2", undefined], ["Reverb", undefined], ["Master", undefined]]);
  assert.deepEqual(bitwig.calls.filter((c) => c[0] !== "createInstrumentTrack"), []);
  assert.equal(app.ask({ id: 2, cmd: "device" }).result.track, "Bass");

  // Back in Night Drive, the next change goes ahead.
  bitwig.app.name.v = "Night Drive";
  assert.deepEqual(app.wait({ id: 3, cmd: "newtrack", name: "Pad", onlyIn: "Night Drive" }).result, { index: 2, name: "Pad" });
});

test("stops a new track before naming it when another project comes to the front while the track listing settles", () => {
  const { bitwig, app } = studio({ browserOnNewTrack: true });
  // Bitwig shows the new track, and while the script waits for the listing to stop changing, just before it names,
  // colours and selects the track and closes Bitwig's browser, someone brings their own project to the front.
  bitwig.switchDuring("newtrack", "My Song", () => bitwig.state.tracks.some((t) => t.name === "Inst 2"));
  assert.deepEqual(app.wait({ id: 1, cmd: "newtrack", name: "Lead", color: "#ff8000", onlyIn: "Night Drive" }),
                   { id: 1, ok: false, error: elsewhere("My Song", "Night Drive") });
  assert.deepEqual(looks(bitwig), [["Bass", undefined], ["Inst 2", undefined], ["Reverb", undefined], ["Master", undefined]]);
  assert.deepEqual(bitwig.calls.map((c) => c[0]), ["createInstrumentTrack"], "nothing selected and the browser not closed");
  assert.equal(bitwig.browser.open.v, true);
  assert.equal(app.ask({ id: 2, cmd: "device" }).result.track, "Bass");

  bitwig.app.name.v = "Night Drive";
  assert.deepEqual(app.wait({ id: 3, cmd: "newtrack", name: "Pad", onlyIn: "Night Drive" }).result, { index: 2, name: "Pad" });
});

test("stops filling pads when another project comes to the front while the samples load", () => {
  const { bitwig, app } = studio();
  app.wait({ id: 1, cmd: "insert", track: 0, at: "start", kind: "bitwig", device: DRUM_MACHINE });
  const dropped = () => bitwig.calls.some((c) => c[0] === "insertFile" && /^pad /.test(c[2] || ""));
  bitwig.switchDuring("pads", "My Song", dropped);
  const kit = [{ note: 36, path: "C:\\Kit\\Kick.wav", name: "Big Kick" }, { note: 38, path: "C:\\Kit\\Snare.wav", name: "Big Snare" }];
  assert.deepEqual(app.wait({ id: 2, cmd: "pads", track: 0, device: 0, pads: kit, onlyIn: "Night Drive" }),
                   { id: 2, ok: false, error: elsewhere("My Song", "Night Drive") });
  // The samples were on their way when the project changed; the pads keep the names Bitwig gave them.
  const pads = bitwig.state.tracks[0].devices[0].pads;
  assert.deepEqual(Object.keys(pads).map((note) => [note, pads[note].name]), [["36", "Kick"], ["38", "Snare"]]);

  bitwig.app.name.v = "Night Drive";
  assert.deepEqual(app.wait({ id: 3, cmd: "pads", track: 0, device: 0, pads: [{ note: 40, path: "C:\\Kit\\Tom.wav", name: "Low Tom" }],
                             onlyIn: "Night Drive" }).result, { filled: [40], replaced: [], skipped: [], failed: [], taken: [] });
  assert.equal(pads[40].name, "Low Tom");
});

test("stops filling pads before dropping the samples when another project comes to the front while the pads catch up", () => {
  // The pad list starts at note 36, so the script first moves it down to note 0.
  const { bitwig, app } = studio({ padScroll: 36 });
  app.wait({ id: 1, cmd: "insert", track: 0, at: "start", kind: "bitwig", device: DRUM_MACHINE });
  // The Drum Machine is selected and its pads start from note 0, and while the script waits for the pads to catch
  // up, just before it drops the samples on them, someone brings their own project to the front.
  bitwig.switchDuring("pads", "My Song", () => bitwig.cursorDevice.drum.v && bitwig.pads.scroll.v === 0);
  const kit = [{ note: 36, path: "C:\\Kit\\Kick.wav", name: "Big Kick" }, { note: 38, path: "C:\\Kit\\Snare.wav", name: "Big Snare" }];
  assert.deepEqual(app.wait({ id: 2, cmd: "pads", track: 0, device: 0, pads: kit, onlyIn: "Night Drive" }),
                   { id: 2, ok: false, error: elsewhere("My Song", "Night Drive") });
  assert.deepEqual(bitwig.calls.filter((c) => /^pad /.test(c[2] || "")), [], "no sample dropped on a pad");
  assert.equal(bitwig.state.tracks[0].devices[0].pads, undefined);

  bitwig.app.name.v = "Night Drive";
  assert.deepEqual(app.wait({ id: 3, cmd: "pads", track: 0, device: 0, pads: [{ note: 40, path: "C:\\Kit\\Tom.wav", name: "Low Tom" }],
                             onlyIn: "Night Drive" }).result, { filled: [40], replaced: [], skipped: [], failed: [], taken: [] });
  assert.equal(bitwig.state.tracks[0].devices[0].pads[40].name, "Low Tom");
});

test("doesn't start a change that waited its turn once another project is in front", () => {
  const { bitwig, app } = studio();
  app.send({ id: 1, cmd: "newtrack", name: "One", onlyIn: "Night Drive" });
  app.send({ id: 2, cmd: "clip", track: 0, path: "C:\\MIDI\\Bass.mid", onlyIn: "Night Drive" });
  app.send({ id: 3, cmd: "pads", track: 0, device: 0, pads: [{ note: 36, path: "C:\\Kit\\Kick.wav" }], onlyIn: "Night Drive" });
  assert.deepEqual(app.take(), [], "they wait their turn");
  bitwig.clock.advance(10);
  bitwig.app.name.v = "My Song";
  bitwig.clock.advance(20000);
  assert.deepEqual(app.take(), [1, 2, 3].map((id) => ({ id, ok: false, error: elsewhere("My Song", "Night Drive") })));
  assert.deepEqual(bitwig.calls.map((c) => c[0]), ["createInstrumentTrack"], "only what the first asked before the change");
  assert.deepEqual(bitwig.state.tracks[0].slots, []);

  bitwig.app.name.v = "Night Drive";
  assert.deepEqual(app.wait({ id: 4, cmd: "clip", track: 0, path: "C:\\MIDI\\Bass.mid", onlyIn: "Night Drive" }).result, { slot: 0 });
});

test("stops a change part way when changes are turned off meanwhile", () => {
  const { bitwig, app } = studio();
  app.send({ id: 1, cmd: "newtrack", name: "Lead", color: "#ff8000" });
  app.send({ id: 2, cmd: "newtrack", name: "Pad" });   // waits its turn
  bitwig.clock.advance(10);
  bitwig.allowChanges(false);
  bitwig.clock.advance(20000);
  const replies = app.take();
  assert.deepEqual(replies.map((r) => [r.id, r.ok]), [[1, false], [2, false]]);
  for (const reply of replies) assert.match(reply.error, CHANGES_OFF);
  assert.deepEqual(looks(bitwig), [["Bass", undefined], ["Inst 2", undefined], ["Reverb", undefined], ["Master", undefined]]);
  assert.deepEqual(bitwig.calls.map((c) => c[0]), ["createInstrumentTrack"]);
});
