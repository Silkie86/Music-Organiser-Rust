// Serves the Bitwig script, running in the stand-in Bitwig, on a TCP port on this computer, with each
// message framed as Bitwig frames them (a 4-byte big-endian length in front). The app's tests use it
// to talk to the real script. Prints "port <number>" once it's listening; stops when stdin closes.
// Each line on stdin changes the stand-in, as if done in Bitwig: {"project": "Other Song", "modified": true}
// opens another project or edits this one, {"presets": false} stops it loading preset files, {"changes":
// false} turns off "Let the app make changes", and {"turn": {"track": "...", "id": "CONTENTS/PID3e9",
// "value": 0.25}} moves a knob of the MO Probe on that track as if the probe turned it itself. It prints
// "ok" once the change is made.
//
// {"switchDuring": "newtrack", "project": "Night Drive"} brings another project to the front part way
// through a change instead of now: the next time a change asked for with that command (newtrack, insert,
// clip, pads, transpose) is running and the script starts to wait for Bitwig the first time, the open
// project's name becomes "Night Drive", as if someone brought their own project to the front just after
// the change began. Only the name changes (the tracks the stand-in shows then stand for that project's).
// It prints "ok" at once, when the line is read.
//
// Like Bitwig, the project has unsaved changes once anything in it changes, until the Save action runs.
// {"ignoreSaves": 2} makes the stand-in ignore the next 2 Saves, leaving the unsaved changes, as Bitwig
// 6.1.3 once did with the test project.
//
//   node serve.js [--raw] [--events]
//     --raw: send without the length in front, to check the app copes either way
//     --events: say on stderr, one JSON line each, when an MO Probe is added to a track ({"event": "probe",
//       "track", "format"}) and when one of its knobs is set from the app ({"event": "set", "track", "id",
//       "value"}), so a stand-in probe can follow, as the real one does inside Bitwig, and each time Save
//       runs ({"event": "save", "ignored"}), so a test can count them
"use strict";

const net = require("node:net");
const { startBitwig } = require("./fake-bitwig");

const raw = process.argv.includes("--raw");
const events = process.argv.includes("--events");
/** A knob as the stand-in shows it once set: its value as a percentage. */
function knob(id, name, value) {
  return { id, name, value, display: `${Math.round(value * 100)}%` };
}
const model = {
  project: "Night Drive",
  tempo: 124,
  // What the app can send in: any preset file, one plugin by its ID, and a Drum Machine (the stand-in's ID).
  presets: true,
  plugins: {
    "VST3:805ECA49563E43EA8119369F0ED6096D": { name: "Serum", plugin: true, kind: "note_to_audio" },
    // MO Probe Instrument by its CLAP ID and its VST3 class ID (native/mo-probe/src/probe.cpp).
    "CLAP:org.musicorganiser.probe.instrument": { name: "MO Probe Instrument", plugin: true, kind: "note_to_audio", probe: "CLAP" },
    "VST3:B0DDF78C9D004BA9B2359D3E7B74D7F1": { name: "MO Probe Instrument", plugin: true, kind: "note_to_audio", probe: "VST3" },
  },
  knobs: {
    // A new Serum's knobs, by the names FL Studio's stand-in gives them, and one that doesn't move when set.
    Serum: [
      { id: "CONTENTS/PID0", name: "Master Volume", value: 0.5, display: "50%" },
      { id: "CONTENTS/PID1", name: "A Octave", value: 0.5, display: "50%" },
      { id: "CONTENTS/PID2", name: "Osc B Level", value: 0.5, display: "50%", locked: true },
      { id: "CONTENTS/PID3e8", name: "Filter Cutoff", value: 0.5, display: "50%" },
    ],
    Sampler: [
      { id: "CONTENTS/GAIN", name: "Gain", value: 0.5, display: "0.0 dB" },
      { id: "CONTENTS/TRANSPOSE", name: "Transpose", value: 0.5, display: "+0.00 st" },
    ],
    // MO Probe's eight knobs at their defaults, from 0 to 1 over each one's range.
    "MO Probe Instrument": [
      [1001, "MO Knob A", 0, 1, 0.25], [1002, "MO Gain", -60, 12, 0], [1003, "MO Frequency", 20, 20000, 440],
      [1004, "MO Steps", 0, 7, 3], [1005, "MO Switch", 0, 1, 0], [1006, "MO Choice", 0, 3, 1],
      [1007, "MO Ramp", 0, 100, 50], [1008, "MO Bypass", 0, 1, 0],
    ].map(([id, name, min, max, value]) => knob("CONTENTS/PID" + id.toString(16), name, (value - min) / (max - min))),
  },
  drumMachine: "d7d7d7d7-0000-4000-8000-000000000001",
  tracks: [
    { name: "Drums", type: "Group", group: true, devices: [] },
    { name: "Kick", type: "Audio", devices: [{ name: "EQ+" }] },
    {
      name: "Bass", type: "Instrument", colour: [1, 0.5, 0], volume: [0.6, "-6.0 dB"],
      devices: [{ name: "Serum", preset: "Reese", plugin: true, kind: "note_to_audio" }, { name: "Compressor" }],
    },
    { name: "Master", type: "Master", devices: [{ name: "Peak Limiter" }] },
  ],
  selectedTrack: "Bass",
  actions: [{ id: "Save", name: "Save", category: "File" }],
};
const bitwig = startBitwig(model);
bitwig.allowChanges(true);
bitwig.cursorDevice.show({ name: "Serum", preset: "Reese", plugin: true, kind: "note_to_audio" }, [
  { id: "CONTENTS/PID0", name: "Master Volume", value: 0.7, display: "-3.1 dB" },
  { id: "CONTENTS/PID3e8", name: "Filter Cutoff", value: 0.25, display: "440 Hz" },
  { id: "CONTENTS/PID3e9", name: "Unreadable", value: NaN, display: "?" },
]);
/** Says what happened on stderr, for a stand-in probe to follow (with --events). */
function tell(event) {
  if (events) process.stderr.write(JSON.stringify(event) + "\n");
}

// Saving clears the unsaved changes, a moment after, as in Bitwig, unless it's one of the Saves to ignore.
let saved = JSON.stringify(bitwig.state.tracks);
let ignoreSaves = 0;
const save = bitwig.app.getAction("Save");
const invokeSave = save.invoke;
save.invoke = () => {
  invokeSave();
  const ignored = ignoreSaves > 0;
  tell({ event: "save", ignored });
  if (ignored) {
    ignoreSaves--;
    return;
  }
  bitwig.clock.schedule(() => {
    saved = JSON.stringify(bitwig.state.tracks);
    bitwig.project.modified.v = false;
  }, 50);
};

// A knob set from the app on the selected device: an MO Probe hears of it.
const setKnob = bitwig.cursorDevice.setDirectParameterValueNormalized.bind(bitwig.cursorDevice);
bitwig.cursorDevice.setDirectParameterValueNormalized = (id, value, resolution) => {
  setKnob(id, value, resolution);
  const device = bitwig.cursorDevice.followed;
  if (device && device.probe) tell({ event: "set", track: bitwig.cursorTrack.values.name.v, id, value: value / (resolution - 1) });
};

// The stand-in's clock, in real time, so the device request's wait for texts ends. Each tick also looks for
// changes to the project (which leave it with unsaved changes) and for MO Probes just added.
const announced = new WeakSet();
setInterval(() => {
  bitwig.clock.advance(20);
  const now = JSON.stringify(bitwig.state.tracks);
  if (now !== saved) {
    saved = now;
    bitwig.project.modified.v = true;
  }
  for (const track of bitwig.state.tracks) {
    for (const device of track.devices) {
      if (!device.probe || announced.has(device)) continue;
      announced.add(device);
      tell({ event: "probe", track: track.name, format: device.probe });
    }
  }
}, 20).unref();

/** The probe turned one of its own knobs: Bitwig's knob follows, and tells the script if it shows that device. */
function turn({ track, id, value }) {
  const spec = bitwig.state.tracks.find((t) => t.name === track);
  const device = spec && spec.devices.find((d) => d.probe);
  const moved = device && (device.knobs || []).find((k) => k.id === id);
  if (!moved) return;
  Object.assign(moved, knob(id, moved.name, value));
  if (bitwig.cursorDevice.followed === device) bitwig.cursorDevice.valueObserver(id, value);
}

const server = net.createServer((socket) => {
  let pending = Buffer.alloc(0);
  let receive = () => {};
  let disconnect = () => {};
  bitwig.attach({
    setReceiveCallback: (cb) => { receive = cb; },
    setDisconnectCallback: (cb) => { disconnect = cb; },
    send(bytes) {
      const body = Buffer.from(bytes);
      if (raw) {
        socket.write(body);
      } else {
        const head = Buffer.alloc(4);
        head.writeUInt32BE(body.length);
        socket.write(Buffer.concat([head, body]));
      }
    },
  });
  socket.on("data", (chunk) => {
    pending = Buffer.concat([pending, chunk]);
    while (pending.length >= 4 && pending.length >= 4 + pending.readUInt32BE(0)) {
      const length = pending.readUInt32BE(0);
      const body = pending.subarray(4, 4 + length);
      pending = pending.subarray(4 + length);
      receive(Array.from(body, (b) => (b > 127 ? b - 256 : b)));
    }
  });
  socket.on("close", () => disconnect());
  socket.on("error", () => {});
});
server.listen(0, "127.0.0.1", () => console.log("port " + server.address().port));

let typed = "";
process.stdin.setEncoding("utf8");
process.stdin.on("data", (chunk) => {
  typed += chunk;
  for (let end = typed.indexOf("\n"); end >= 0; end = typed.indexOf("\n")) {
    const line = typed.slice(0, end).trim();
    typed = typed.slice(end + 1);
    if (!line) continue;
    const change = JSON.parse(line);
    // With switchDuring, "project" is the one that comes to the front later, part way through the change.
    if ("switchDuring" in change) bitwig.switchDuring(change.switchDuring, change.project);
    else if ("project" in change) bitwig.app.name.v = change.project;
    if ("modified" in change) bitwig.project.modified.v = !!change.modified;
    if ("presets" in change) model.presets = change.presets;
    if ("changes" in change) bitwig.allowChanges(!!change.changes);
    if ("ignoreSaves" in change) ignoreSaves = Number(change.ignoreSaves) || 0;
    if ("turn" in change) turn(change.turn);
    console.log("ok");
  }
});
process.stdin.on("end", () => process.exit(0));
