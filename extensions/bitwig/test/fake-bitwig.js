// A stand-in for the parts of Bitwig's controller API the Music Organiser script uses, so the script
// can be run and checked without Bitwig. Like Bitwig, reading a value that init() didn't mark as
// interested fails, and changes the script asks for happen a little later (on the stand-in's clock),
// reaching the script bit by bit, as Bitwig's do.
"use strict";

const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");

const SCRIPT = path.join(__dirname, "..", "MusicOrganiser.control.js");
const SAMPLER = "468bc14b-b2e7-45a1-9666-e83117fe404e";
const NOTE_TRANSPOSE = "0815cd9e-3a31-4429-a268-dabd952a3b68";
const AUDIO = /\.(wav|aiff?|flac|mp3|ogg)$/i;
const PRESET = /\.(bwpreset|vstpreset|fxb|fxp)$/i;

/** A value the script reads. The script's set() goes through `onSet`, when given, so it can take effect later. */
class Value {
  constructor(v, onSet) {
    this.v = v;
    this.interested = false;
    this.onSet = onSet;
  }
  markInterested() {
    this.interested = true;
  }
  get() {
    if (!this.interested) throw new Error("read a value that wasn't marked as interested");
    return this.v;
  }
  set(v) {
    if (this.onSet) this.onSet(v);
    else this.v = v;
  }
}

class Parameter extends Value {
  constructor(v, display) {
    super(v);
    this.display = new Value(display);
  }
  getRaw() {
    return this.get();
  }
  displayedValue() {
    return this.display;
  }
}

class Colour {
  constructor(r, g, b, onSet) {
    Object.assign(this, { r, g, b, interested: false, onSet });
  }
  markInterested() {
    this.interested = true;
  }
  check() {
    if (!this.interested) throw new Error("read a colour that wasn't marked as interested");
  }
  red() { this.check(); return this.r; }
  green() { this.check(); return this.g; }
  blue() { this.check(); return this.b; }
  set(r, g, b) { this.onSet([r, g, b]); }
}

class Bank {
  constructor(items) {
    this.items = items;
    this.count = new Value(0);
  }
  getItemAt(i) {
    return this.items[i];
  }
  itemCount() {
    return this.count;
  }
}

/** Logs an insertion point's calls, and hands each to `act` (if given) to carry out. */
function insertionPoint(calls, where, act) {
  const point = {};
  for (const m of ["insertVST3Device", "insertVST2Device", "insertCLAPDevice", "insertBitwigDevice", "insertFile"])
    point[m] = (arg) => {
      calls.push(where ? [m, arg, where] : [m, arg]);
      if (act) act(m, arg);
    };
  return point;
}

/** A parameter of a specific Bitwig device: it exists while the device it looks at is that device. */
class SpecificParam extends Parameter {
  constructor(owner, id, param) {
    super(0, "");
    Object.assign(this, { owner, id, param });
    this.present = new Value(false);
  }
  exists() { return this.present; }
  setRaw(v) { this.owner.setParam(this.id, this.param, v); }
}

class Device {
  constructor(world) {
    this.world = world;
    this.spec = null;
    this.specifics = [];
    this.values = {
      exists: new Value(false), name: new Value(""), presetName: new Value(""),
      isPlugin: new Value(false), deviceType: new Value(""), isEnabled: new Value(true), sampleName: new Value(""),
    };
  }
  load(spec) {
    this.spec = spec || null;
    const v = this.values;
    v.exists.v = !!spec;
    v.name.v = spec ? spec.name : "";
    v.presetName.v = spec ? spec.preset || "" : "";
    v.isPlugin.v = !!(spec && spec.plugin);
    v.deviceType.v = spec ? spec.kind || "audio_to_audio" : "";
    v.isEnabled.v = !spec || spec.enabled !== false;
    v.sampleName.v = spec && spec.sample ? path.basename(spec.sample.replace(/\\/g, "/")) : "";
    for (const p of this.specifics) {
      const mine = !!spec && spec.id === p.id;
      p.present.v = mine;
      p.v = mine ? (spec.params || {})[p.param] || 0 : 0;
      p.display.v = mine ? formatParam(p.param, p.v) : "";
    }
  }
  exists() { return this.values.exists; }
  name() { return this.values.name; }
  presetName() { return this.values.presetName; }
  isPlugin() { return this.values.isPlugin; }
  deviceType() { return this.values.deviceType; }
  isEnabled() { return this.values.isEnabled; }
  sampleName() { return this.values.sampleName; }
  createSpecificBitwigDevice(id) {
    const key = typeof id === "string" ? id : id.uuid;
    return { createParameter: (param) => {
      const p = new SpecificParam(this, key, param);
      this.specifics.push(p);
      return p;
    } };
  }
  /** Bitwig keeps a value in the parameter's range; the Sampler's real range isn't known, so ±48 stands in. */
  setParam(id, param, v) {
    const spec = this.spec;
    this.world.later(20, () => {
      if (!spec || spec.id !== id) return;
      spec.params = spec.params || {};
      spec.params[param] = Math.max(-48, Math.min(48, param === "COARSE" ? Math.round(v) : v));
    });
  }
}

function formatParam(param, v) {
  return param === "COARSE" ? `${v > 0 ? "+" : ""}${v} st` : `${v >= 0 ? "+" : ""}${v.toFixed(2)} st`;
}

/** The device selected in Bitwig, with Bitwig's "direct parameter" observers. */
class CursorDevice extends Device {
  constructor(world) {
    super(world);
    this.clock = world.clock;
    this.params = [];
    this.observed = undefined;
    this.observedHistory = [];
    this.selected = [];
    this.setCalls = [];
    this.where = new Value(-1);
    this.drum = new Value(false);
  }
  addDirectParameterIdObserver(cb) { this.idObserver = cb; }
  addDirectParameterNameObserver(maxChars, cb) { this.nameObserver = cb; }
  addDirectParameterNormalizedValueObserver(cb) { this.valueObserver = cb; }
  addDirectParameterValueDisplayObserver(maxChars, cb) {
    this.displayObserver = cb;
    const self = this;
    return {
      setObservedParameterIds(ids) {
        self.observed = ids;
        self.observedHistory.push(ids);
        if (!ids) return;
        // Bitwig sends the texts a moment later; parameters without one never answer.
        self.clock.schedule(() => {
          if (self.observed !== ids) return;
          for (const p of self.params)
            if (p.display !== undefined && ids.indexOf(p.id) >= 0) self.displayObserver(p.id, p.display);
        }, 30);
      },
    };
  }
  /** Selects a device in "Bitwig" that isn't on any track: its parameters arrive through the observers. */
  show(spec, params) {
    this.world.cursor.device = undefined;
    this.followed = undefined;
    this.load(spec);
    this.where.v = spec ? 0 : -1;
    this.drum.v = !!(spec && spec.drum);
    this.announce(params || []);
  }
  /**
   * Bitwig's direct parameters for the device on a track the cursor is on now: the `knobs` its spec has
   * ({ id, name, value, display }), or none. Only a different device announces them again.
   */
  follow(spec) {
    if (spec === this.followed) return;
    this.followed = spec;
    this.announce(spec && spec.knobs ? spec.knobs : []);
  }
  announce(params) {
    this.params = params;
    this.idObserver(this.params.map((p) => p.id));
    for (const p of this.params) {
      this.nameObserver(p.id, p.name);
      this.valueObserver(p.id, p.value);
    }
  }
  position() { return this.where; }
  hasDrumPads() { return this.drum; }
  createDrumPadBank(n) {
    this.world.pads = new DrumPadBank(this.world, n);
    return this.world.pads;
  }
  selectDevice(device) {
    this.selected.push(device);
    this.world.selectDevice(device);
  }
  /** The value changes a moment later, and Bitwig tells the script, as it does for a knob turned by hand. A `locked` knob stays put. */
  setDirectParameterValueNormalized(id, value, resolution) {
    this.setCalls.push([id, value, resolution]);
    const knob = this.params.find((p) => p.id === id);
    if (!knob || knob.locked) return;
    this.world.later(20, () => {
      knob.value = value / (resolution - 1);
      if (knob.display !== undefined) knob.display = `${Math.round(knob.value * 100)}%`;
      this.valueObserver(id, knob.value);
    });
  }
}

class DrumPad {
  constructor(world, index) {
    this.world = world;
    this.index = index;
    this.present = new Value(false);
    this.title = new Value("", (name) => world.later(20, () => {
      const pad = world.padAt(index);
      if (pad) pad.name = name;
    }));
    /** The pad's devices, as many as the extension asks to see: the first can be replaced by a sample. */
    this.chain = new Bank([]);
    this.point = insertionPoint(world.calls, `pad ${index}`, (m, arg) => {
      const device = world.padDevice();
      if (m !== "insertFile" || !AUDIO.test(arg) || !device) return;
      world.later(40, () => {
        const note = world.padScroll + index;
        if (note >= 128) return;
        device.pads = device.pads || {};
        if (!device.pads[note]) device.pads[note] = { name: baseName(arg), devices: [] };
        device.pads[note].devices.push({ name: "Sampler", id: SAMPLER, kind: "note_to_audio", sample: arg });
      });
    });
  }
  exists() { return this.present; }
  name() { return this.title; }
  createDeviceBank(n) {
    const devices = [new PadDevice(this.world, this.index)];
    for (let i = 1; i < n; i++) devices.push(new Device(this.world));
    this.chain = new Bank(devices);
    return this.chain;
  }
  insertionPoint() { return this.point; }
}

/** A pad's first device. A sample put in its place takes the place of the pad's first device as a Sampler. */
class PadDevice extends Device {
  constructor(world, index) {
    super(world);
    this.replace = insertionPoint(world.calls, `pad ${index} first device`, (m, arg) => {
      const device = world.padDevice();
      if (m !== "insertFile" || !AUDIO.test(arg) || !device) return;
      world.later(40, () => {
        const pad = device.pads && device.pads[world.padScroll + index];
        if (pad && pad.devices.length) pad.devices[0] = { name: "Sampler", id: SAMPLER, kind: "note_to_audio", sample: arg };
      });
    });
  }
  replaceDeviceInsertionPoint() { return this.replace; }
}

class DrumPadBank extends Bank {
  constructor(world, n) {
    super(Array.from({ length: n }, (_, i) => new DrumPad(world, i)));
    this.indication = true;
    this.scroll = new Value(world.padScroll, (v) => world.later(20, () => { world.padScroll = Math.max(0, Math.min(127, v)); }));
  }
  setIndication(on) { this.indication = on; }
  scrollPosition() { return this.scroll; }
}

class Track {
  constructor(world, index) {
    this.world = world;
    this.index = index;
    const later = (apply) => (v) => world.later(20, () => {
      const spec = index === null ? world.cursorSpec() : world.state.tracks[index];
      if (spec) apply(spec, v);
    });
    this.values = {
      exists: new Value(false), name: new Value("", later((s, v) => { s.name = v; })), trackType: new Value(""),
      isGroup: new Value(false), isActivated: new Value(true), mute: new Value(false), solo: new Value(false),
      arm: new Value(false),
    };
    this.colour = new Colour(0, 0, 0, later((s, rgb) => { s.colour = rgb; }));
    this.vol = new Parameter(0.79, "0.0 dB");
    this.panning = new Parameter(0.5, "C");
    this.devices = null;
    this.slots = null;
    this.isCursor = null;
    if (index === null) return;
    const chain = (at) => insertionPoint(world.calls, `track ${index} ${at}`, (m, arg) => world.insert(index, at, m, arg));
    this.start = chain("start");
    this.end = chain("end");
  }
  load(spec) {
    const v = this.values;
    v.exists.v = !!spec;
    if (this.isCursor) this.isCursor.v = this.index !== null && this.world.cursor.track === this.index;
    if (!spec) return;
    v.name.v = spec.name;
    v.trackType.v = spec.type || "Instrument";
    v.isGroup.v = !!spec.group;
    v.mute.v = !!spec.mute;
    if (spec.colour) [this.colour.r, this.colour.g, this.colour.b] = spec.colour;
    if (spec.volume) [this.vol.v, this.vol.display.v] = spec.volume;
    if (this.devices) {
      const devices = spec.devices || [];
      this.devices.items.forEach((d, i) => d.load(devices[i]));
      this.devices.count.v = devices.length;
    }
    if (this.slots) {
      this.slots.items.forEach((slot, s) => {
        slot.present.v = s < this.world.state.scenes;
        slot.content.v = !!(spec.slots || [])[s];
      });
    }
  }
  exists() { return this.values.exists; }
  name() { return this.values.name; }
  trackType() { return this.values.trackType; }
  isGroup() { return this.values.isGroup; }
  isActivated() { return this.values.isActivated; }
  mute() { return this.values.mute; }
  solo() { return this.values.solo; }
  arm() { return this.values.arm; }
  color() { return this.colour; }
  volume() { return this.vol; }
  pan() { return this.panning; }
  createDeviceBank(n) {
    this.devices = new Bank(Array.from({ length: n }, () => new Device(this.world)));
    return this.devices;
  }
  createEqualsValue(other) {
    if (other !== this.world.cursorTrack) throw new Error("expected a comparison with the cursor track");
    this.isCursor = new Value(false);
    return this.isCursor;
  }
  startOfDeviceChainInsertionPoint() { return this.start; }
  endOfDeviceChainInsertionPoint() { return this.end; }
  clipLauncherSlotBank() {
    const world = this.world;
    const t = this.index;
    this.slots = new Bank(Array.from({ length: world.scenes }, (_, s) => ({
      present: new Value(false),
      content: new Value(false),
      exists() { return this.present; },
      hasContent() { return this.content; },
      replaceInsertionPoint: () => insertionPoint(world.calls, `track ${t} slot ${s}`, (m, arg) => {
        if (m !== "insertFile" || !/\.midi?$/i.test(arg)) return;
        const at = world.state.tracks[t];
        world.later(40, () => {
          at.slots = at.slots || [];
          at.slots[s] = { file: arg };
          world.state.scenes = Math.max(world.state.scenes, s + 1);
        });
      }),
    })));
    return this.slots;
  }
}

/** A fresh copy of a track as the stand-in project keeps it. */
function trackState(spec) {
  return {
    ...spec,
    devices: (spec.devices || []).map((d) => ({ ...d })),
    slots: (spec.slots || []).slice(),
  };
}

function baseName(file) {
  return path.basename(file.replace(/\\/g, "/")).replace(/\.[^.]*$/, "");
}

/**
 * Runs the script in a fake Bitwig with the given project, and returns handles to poke at it.
 *
 * Besides the project (tracks, tempo, actions...), `model` can say what Bitwig would make of things the
 * app inserts: `files` (path -> the device a preset loads), `presets` (true to load any preset file, as
 * the plugin its name starts with: "Serum - Lead.vstpreset"), `plugins` ("VST3:<id>", "VST2:<number>",
 * "CLAP:<id>" -> the device), `drumMachine` (the UUID that adds a Drum Machine here), `scenes` (how many
 * scenes the project has), `padScroll` (where the pad list starts), `drumPads` (the devices on every pad of
 * a new Drum Machine, as a saved default Drum Machine can have: Solomon's has an empty Sampler on each) and
 * `browserOnNewTrack` (whether a new track opens Bitwig's browser). A device can list its `knobs`, which the script sees once it's selected,
 * and `knobs` can give them by device name ("Serum" -> its knobs) for whatever the app inserts.
 * `portInUse` is the port Bitwig gives the link when 19791 is taken, and `portUnknown` how many times
 * Bitwig says -1 when asked which port it gave (Bitwig 6.1.3 does, straight after making the connection).
 */
function startBitwig(model = {}) {
  const clock = {
    now: 0,
    tasks: [],
    schedule(fn, ms) { this.tasks.push({ at: this.now + ms, fn }); },
    advance(ms) {
      const end = this.now + ms;
      for (;;) {
        this.tasks.sort((a, b) => a.at - b.at);
        const next = this.tasks[0];
        if (!next || next.at > end) break;
        this.tasks.shift();
        this.now = next.at;
        next.fn();
      }
      this.now = end;
    },
  };
  const calls = [];
  const errors = [];
  let portUnknown = model.portUnknown || 0;
  const popups = [];
  const state = { tracks: (model.tracks || []).map(trackState), scenes: model.scenes === undefined ? 8 : model.scenes, count: null };
  const builtIn = {
    [SAMPLER]: { name: "Sampler", id: SAMPLER, kind: "note_to_audio", params: { TRANSPOSE: 0 } },
    [NOTE_TRANSPOSE]: { name: "Note Transpose", id: NOTE_TRANSPOSE, kind: "note_to_note", params: { COARSE: 0 } },
  };
  if (model.drumMachine) {
    const machine = { name: "Drum Machine", id: model.drumMachine, kind: "note_to_audio", drum: true };
    if (model.drumPads) {
      machine.pads = {};
      for (let note = 0; note < 128; note++)
        machine.pads[note] = { name: model.drumPads[0].name, devices: model.drumPads.map((d) => ({ ...d })) };
    }
    builtIn[model.drumMachine] = machine;
  }
  const browser = { open: new Value(false), exists() { return this.open; }, cancel() { calls.push(["cancelBrowser"]); this.open.v = false; } };

  const world = {
    clock, calls, state,
    scenes: 0,
    pads: null,
    padScroll: model.padScroll || 0,
    /**
     * What's selected: a track's index and the device on it (null for none, undefined for one shown by
     * itself with show()), and the device whose pads the pad list shows.
     */
    cursor: { track: null, device: undefined, padsFrom: null },
    cursorTrack: null,
    /** Makes a change a moment from now, then tells the script, as Bitwig does. */
    later(ms, change) {
      clock.schedule(() => {
        change();
        sync();
      }, ms);
    },
    cursorSpec() { return world.cursor.track === null ? null : state.tracks[world.cursor.track]; },
    selectTrack(track) {
      world.later(20, () => {
        const spec = track.index === null ? null : state.tracks[track.index];
        if (!spec || world.cursor.track === track.index) return;
        world.cursor.track = track.index;
        world.cursor.device = spec.devices[0] || null;
      });
    },
    selectDevice(device) {
      const owner = tracks.items.find((t) => t.devices && t.devices.items.includes(device));
      if (!owner) return;
      const index = owner.devices.items.indexOf(device);
      world.later(20, () => {
        const spec = state.tracks[owner.index] && state.tracks[owner.index].devices[index];
        if (!spec) return;
        world.cursor.track = owner.index;
        world.cursor.device = spec;
      });
      // The pads follow a moment after the device.
      clock.schedule(() => {
        world.cursor.padsFrom = world.cursor.device;
        sync();
      }, 60);
    },
    padDevice() {
      const d = world.cursor.padsFrom;
      return d && d.drum ? d : null;
    },
    padAt(index) {
      const d = world.padDevice();
      return d && d.pads ? d.pads[world.padScroll + index] : null;
    },
    /** Something inserted into track `t`'s chain: what Bitwig would make of it, if anything. */
    insert(t, at, method, arg) {
      let spec = null;
      if (method === "insertFile") {
        spec = (model.files || {})[arg] || (AUDIO.test(arg) ? { ...builtIn[SAMPLER], preset: baseName(arg) } : null);
        if (!spec && model.presets && PRESET.test(arg)) {
          const [plugin, ...name] = baseName(arg).split(" - ");
          spec = { name: plugin, preset: name.join(" - ") || plugin, plugin: true, kind: "note_to_audio" };
        }
      }
      else if (method === "insertBitwigDevice") spec = builtIn[typeof arg === "string" ? arg : arg.uuid];
      else {
        const format = { insertVST3Device: "VST3", insertVST2Device: "VST2", insertCLAPDevice: "CLAP" }[method];
        spec = (model.plugins || {})[`${format}:${arg}`];
      }
      if (!spec) return;
      if (!spec.knobs && model.knobs && model.knobs[spec.name]) spec = { ...spec, knobs: model.knobs[spec.name] };
      const track = state.tracks[t];
      world.later(40, () => {
        const device = JSON.parse(JSON.stringify(spec));
        if (at === "start") track.devices.unshift(device);
        else track.devices.push(device);
      });
    },
  };

  const actions = (model.actions || []).map((a) => ({
    getId: () => a.id, getName: () => a.name, getMenuItemText: () => a.menu || a.name,
    getCategory: () => (a.category ? { getName: () => a.category } : null),
    invoke: () => calls.push(["invoke", a.id]),
  }));
  const app = {
    name: new Value(model.project || "Untitled"),
    engine: new Value(true),
    projectName() { return this.name; },
    hasActiveEngine() { return this.engine; },
    getActions: () => actions,
    getAction: (id) => actions.find((a) => a.getId() === id) || null,
    createInstrumentTrack(position) {
      calls.push(["createInstrumentTrack", position]);
      // The count changes first; the track itself shows up a moment later, after the last main track.
      world.later(30, () => { state.count = state.tracks.length + 1; });
      world.later(60, () => {
        state.count = null;
        const main = state.tracks.findIndex((t) => t.type === "Effect" || t.type === "Master");
        const at = main < 0 ? state.tracks.length : main;
        const instruments = state.tracks.filter((t) => (t.type || "Instrument") === "Instrument").length;
        state.tracks.splice(at, 0, trackState({ name: `Inst ${instruments + 1}`, type: "Instrument" }));
        if (world.cursor.track !== null && world.cursor.track >= at) world.cursor.track++;
        if (model.browserOnNewTrack) browser.open.v = true;
      });
    },
  };
  const project = { modified: new Value(!!model.modified), isModified() { return this.modified; } };
  const transport = {
    tempoValue: new Parameter(model.tempo || 120, ""),
    signature: new Value(model.timeSignature || "4/4"),
    playing: new Value(false),
    position: new Value(0),
    tempo() { return this.tempoValue; },
    timeSignature() { return this.signature; },
    isPlaying() { return this.playing; },
    getPosition() { return this.position; },
  };
  const cursorDevice = new CursorDevice(world);
  const cursorTrack = new Track(world, null);
  world.cursorTrack = cursorTrack;
  cursorTrack.selectChannel = (track) => {
    calls.push(["selectChannel", track]);
    world.selectTrack(track);
  };
  cursorTrack.createCursorDevice = () => cursorDevice;
  const cursorPoint = insertionPoint(calls);
  cursorTrack.endOfDeviceChainInsertionPoint = () => cursorPoint;
  let tracks = null;
  let changes = null;
  let connect = null;
  let defined = null;
  let api = null;
  /** Projects a test asked to bring to the front part way through a change (see switchDuring), and the change last seen waiting. */
  const during = [];
  let lastJob = null;

  /** The script has just started to wait for Bitwig: switches the project if a test asked for it at this point of the change running. */
  function waiting() {
    const job = context.currentJob;   // the script's own: the change it's running, if any
    if (!job) return;
    const first = job !== lastJob;
    lastJob = job;
    for (const d of during.slice()) {
      if (d.cmd !== job.request.cmd || !(d.when ? d.when() : first)) continue;
      during.splice(during.indexOf(d), 1);
      app.name.v = d.project;
    }
  }

  /** Tells the script how things are now, as Bitwig's observers do. */
  function sync() {
    tracks.count.v = state.count === null ? state.tracks.length : state.count;
    tracks.items.forEach((t, i) => t.load(state.tracks[i]));
    const selected = world.cursorSpec();
    if (selected) {
      cursorTrack.values.exists.v = true;
      cursorTrack.values.name.v = selected.name;
    }
    const device = world.cursor.device;
    if (device !== undefined) {
      cursorDevice.load(device);
      cursorDevice.where.v = device && selected ? selected.devices.indexOf(device) : -1;
      cursorDevice.drum.v = !!(device && device.drum);
      cursorDevice.follow(device);
    }
    if (world.pads) {
      const from = world.padDevice();
      world.pads.scroll.v = world.padScroll;
      world.pads.items.forEach((pad, i) => {
        const p = from && from.pads ? from.pads[world.padScroll + i] : null;
        pad.present.v = !!p;
        pad.title.v = p ? p.name : "";
        pad.chain.count.v = p ? p.devices.length : 0;
        pad.chain.items.forEach((d, n) => d.load(p && n < p.devices.length ? p.devices[n] : null));
      });
    }
  }

  const host = {
    defineController: (...args) => { defined = args; },
    defineMidiPorts: () => {},
    createApplication: () => app,
    getProject: () => project,
    createTransport: () => transport,
    createTrackBank(n, sends, scenes, flat) {
      if (!flat) throw new Error("expected a flat track list");
      world.scenes = scenes;
      tracks = new Bank(Array.from({ length: n }, (_, i) => new Track(world, i)));
      return tracks;
    },
    createCursorTrack: (id, name, sends, scenes, follow) => {
      if (!follow) throw new Error("expected a cursor that follows the selection");
      return cursorTrack;
    },
    createPopupBrowser: () => browser,
    getPreferences: () => ({
      getBooleanSetting(label, category, initial) {
        changes = new Value(initial);
        return changes;
      },
    }),
    createRemoteConnection: (name, port) => ({
      setClientConnectCallback(cb) { connect = cb; },
      getPort: () => {
        if (portUnknown > 0) {
          portUnknown--;
          return -1;
        }
        return model.portInUse || port;
      },
    }),
    showPopupNotification: (text) => popups.push(text),
    getHostProduct: () => "Bitwig Studio",
    getHostVersion: () => "6.1.2",
    getHostApiVersion: () => 25,
    getPlatformType: () => "WINDOWS",
    scheduleTask: (fn, ms) => {
      clock.schedule(fn, ms);
      waiting();
    },
  };

  const sandbox = {
    host,
    loadAPI: (v) => { api = v; },
    println: () => {},
    errorln: (text) => errors.push(text),
    Date: { now: () => clock.now },
  };
  if (model.java) sandbox.Java = { type: (name) => ({ fromString: (s) => ({ uuid: s, javaType: name }) }) };
  const context = vm.createContext(sandbox);
  vm.runInContext(fs.readFileSync(SCRIPT, "utf8"), context, { filename: "MusicOrganiser.control.js" });
  context.init();

  // Fill in the project after init(), as Bitwig does.
  sync();
  cursorTrack.load({ name: model.selectedTrack || "", devices: [] });
  cursorTrack.values.exists.v = model.selectedTrack !== undefined;
  const selectedIndex = state.tracks.findIndex((t) => t.name === model.selectedTrack);
  if (selectedIndex >= 0) {
    world.cursor.track = selectedIndex;
    sync();
  }

  return {
    clock, calls, errors, popups, tracks, cursorTrack, cursorDevice, app, project, context, state, browser,
    get api() { return api; },
    get defined() { return defined; },
    get pads() { return world.pads; },
    allowChanges(on) { changes.set(on); },
    /** Tells the script about changes a test made to `state` itself. */
    refresh() { sync(); },
    /**
     * Brings another project to the front part way through the next change asked for with `cmd` ("newtrack",
     * "pads"...), as if someone did it in Bitwig meanwhile: once the script, running that change, first waits
     * for Bitwig, or, with `when`, at its first wait after `when()` comes true (say, once samples are dropped
     * on pads). Only the project's name changes: the tracks the stand-in shows from then on stand for that
     * project's, as Bitwig's track bank follows the project in front.
     */
    switchDuring(cmd, project, when) { during.push({ cmd, project, when }); },
    /** Hands the script a connection, as Bitwig does when the app connects. */
    attach(connection) { connect(connection); },
    /** Connects as the app would, and returns a way to send requests and read what comes back. */
    connect() {
      const received = [];
      let receive = null;
      let disconnect = null;
      connect({
        setReceiveCallback: (cb) => { receive = cb; },
        setDisconnectCallback: (cb) => { disconnect = cb; },
        send(bytes) {
          if (!Array.isArray(bytes)) throw new Error("expected an array of bytes");
          for (const b of bytes)
            if (!Number.isInteger(b) || b < 0 || b > 127) throw new Error(`expected ASCII bytes, got ${b}`);
          received.push(JSON.parse(Buffer.from(bytes).toString("ascii")));
        },
      });
      let read = 0;
      return {
        received,
        /** Messages that arrived since the last call. */
        take() {
          const fresh = received.slice(read);
          read = received.length;
          return fresh;
        },
        /** Sends a request (an object, or raw text) as Bitwig would hand it over: signed bytes. */
        send(request) {
          const text = typeof request === "string" ? request : JSON.stringify(request);
          receive(Array.from(Buffer.from(text, "utf8"), (b) => (b > 127 ? b - 256 : b)));
        },
        /** Sends a request and returns the one reply to it. */
        ask(request) {
          this.send(request);
          const fresh = this.take();
          if (fresh.length !== 1) throw new Error(`expected one reply, got ${JSON.stringify(fresh)}`);
          return fresh[0];
        },
        /** Sends a request, lets the stand-in's clock run (for changes that wait) and returns the reply. */
        wait(request, ms = 10000) {
          this.send(request);
          clock.advance(ms);
          const fresh = this.take();
          if (fresh.length !== 1) throw new Error(`expected one reply, got ${JSON.stringify(fresh)}`);
          return fresh[0];
        },
        close() { disconnect(); },
      };
    },
  };
}

module.exports = { startBitwig, SCRIPT, SAMPLER, NOTE_TRANSPOSE };
