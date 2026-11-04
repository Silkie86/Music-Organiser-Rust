// Music Organiser link for Bitwig Studio.
//
// Lets the Music Organiser app see what's in the open Bitwig project: its tracks and devices, the
// selected device's parameters (with the IDs Bitwig saves them under) and Bitwig's actions. With
// "Let the app make changes" turned on in this controller's settings, the app can also add tracks,
// set a parameter, insert a plugin, device, preset or sample, put a MIDI file in a clip slot, fill a
// Drum Machine's pads, and run an action such as Save. That's how the app sends an FL Studio channel
// or a kit into Bitwig, and how it learns Bitwig's file format: change one thing, save, and compare.
//
// Installing: Settings in the app copies this file into Bitwig's Controller Scripts folder. Then in
// Bitwig: Settings > Controllers > Add controller > Music Organiser > DAW link.
//
// The app connects to port 19791 on this computer and sends JSON requests, one per message; each
// message has a 4-byte big-endian length in front (Bitwig's remote connections work that way).
// The commands are described in extensions/README.md.
//
// Plain ES5 (var and function only), which every Bitwig version's script engine runs.
// @ts-check

loadAPI(18);
host.defineController("Music Organiser", "DAW link", "2.0", "ed35e3c7-c544-49ed-936d-c236b0c9b73b", "Music Organiser");
host.defineMidiPorts(0, 0);

var PORT = 19791;
var LINK_VERSION = 2;
/** Tracks the project listing covers (a flat list: tracks inside groups, effect tracks and the master). */
var MAX_TRACKS = 128;
/** Devices listed per track: the top level of its device chain. */
var MAX_DEVICES = 16;
/** Clip launcher slots per track the app can fill: those of the first scenes. */
var SCENES = 8;
/** A Drum Machine's pads: one for each MIDI note. */
var DRUM_PADS = 128;
/** How many of each pad's devices the link sees: enough to tell an empty Sampler with effects after it from a pad that plays. */
var PAD_DEVICES = 4;
/** Devices at the start of each track whose pitch the app can set (a Sampler or a Note Transpose). */
var TUNED_DEVICES = 2;
/** How long to wait for the selected device's parameter values to arrive, in milliseconds. */
var DEVICE_WAIT = 2000;
/** How long to wait for Bitwig to make a change the app asked for (a track, a device, a clip). */
var CHANGE_WAIT = 5000;
/** When to ask Bitwig again which port it gave the link, if it didn't say at first (milliseconds). */
var PORT_CHECK_DELAY = 1000;
/** Bitwig's own devices the app tunes, by the IDs Bitwig saves them under. */
var SAMPLER = "468bc14b-b2e7-45a1-9666-e83117fe404e";
var NOTE_TRANSPOSE = "0815cd9e-3a31-4429-a268-dabd952a3b68";
var CHANGES_OFF = "Changes are turned off. To let the app change things, turn on \"Let the app make changes\" " +
                  "in Bitwig: Settings > Controllers > Music Organiser.";

/** @type {API.Application} */ var app;
/** @type {API.Project} */ var project;
/** @type {API.Transport} */ var transport;
/** @type {API.TrackBank} */ var tracks;
/** @type {API.DeviceBank[]} */ var trackDevices = [];
/** Where things go on each track: the start and end of its device chain. @type {API.InsertionPoint[]} */
var trackStarts = [];
/** @type {API.InsertionPoint[]} */ var trackEnds = [];
/** Each track's clip launcher slots, and what replaces their contents. @type {API.ClipLauncherSlot[][]} */
var trackSlots = [];
/** @type {API.InsertionPoint[][]} */ var slotInserts = [];
/** The pitch of the first devices on each track, for when they're a Sampler or a Note Transpose. */
/** @type {{ sampler: API.Parameter, transpose: API.Parameter }[][]} */ var trackTunings = [];
/** Whether each track is the selected one. @type {API.BooleanValue[]} */ var trackSelected = [];
/** @type {API.CursorTrack} */ var cursorTrack;
/** @type {API.PinnableCursorDevice} */ var cursorDevice;
/** @type {API.DirectParameterValueDisplayObserver} */ var displayObserver;
/** The selected device's drum pads, when it's a Drum Machine. @type {API.DrumPadBank} */ var drumPads;
/** @type {API.DrumPad[]} */ var padItems = [];
/** Each pad's devices, the first of them, and where to put a sample in place of that one. @type {API.DeviceBank[]} */ var padDevices = [];
/** @type {API.Device[]} */ var padFirst = [];
/** @type {API.InsertionPoint[]} */ var padInserts = [];
/** @type {API.InsertionPoint[]} */ var padReplace = [];
/** @type {API.PopupBrowser} */ var popupBrowser;
/** @type {API.SettableBooleanValue} */ var changesSetting;
/** @type {API.RemoteSocket} */ var socket;
/** The port Bitwig gave the link; 0 or less while Bitwig hasn't said (see knownPort). */
var port = PORT;
/** Whether Bitwig has been told the port is taken (once is enough). */
var portWarned = false;

/**
 * The selected device's parameters, as Bitwig reports them. Display texts are only asked for while a
 * request waits for them, since Bitwig advises against watching them all the time.
 * @type {{ ids: string[], names: Object<string, string>, values: Object<string, number>, displays: Object<string, string> }}
 */
var params = { ids: [], names: {}, values: {}, displays: {} };

/** @typedef {{ connection: API.RemoteConnection, open: boolean }} Client */
/** @type {Client[]} */ var clients = [];
/** Requests waiting for the selected device's parameter values. @type {{ client: Client, id: any }[]} */
var deviceWaiters = [];
var deviceWaitStart = 0;

/**
 * Ends a change: with its result, or with an error message for the app.
 * @typedef {function(any, string=): void} Finish
 */
/**
 * A change that waits for Bitwig, and whether it needs changes turned on.
 * @typedef {{ client: Client, id: any, request: any, run: function(any, Finish): void, changes: boolean }} Job
 */
/** Changes that wait for Bitwig, run one at a time in the order they were asked for. @type {Job[]} */
var jobs = [];
/** The change running now, if any: each of its steps after a wait checks it may still go ahead. @type {Job} */
var currentJob = null;

function init() {
    app = host.createApplication();
    app.projectName().markInterested();
    app.hasActiveEngine().markInterested();
    project = host.getProject();
    project.isModified().markInterested();

    transport = host.createTransport();
    transport.tempo().markInterested();
    transport.timeSignature().markInterested();
    transport.isPlaying().markInterested();
    transport.getPosition().markInterested();

    // The track and device selected in Bitwig.
    cursorTrack = host.createCursorTrack("MO_LINK", "Music Organiser", 0, 0, true);
    cursorTrack.exists().markInterested();
    cursorTrack.name().markInterested();
    cursorDevice = cursorTrack.createCursorDevice();
    watchDevice(cursorDevice);
    cursorDevice.position().markInterested();
    cursorDevice.hasDrumPads().markInterested();
    cursorDevice.addDirectParameterIdObserver(function (ids) {
        var list = [];
        for (var i = 0; i < ids.length; i++) list.push(String(ids[i]));
        params.ids = list;
        params.names = keep(params.names, list);
        params.values = keep(params.values, list);
        params.displays = keep(params.displays, list);
    });
    cursorDevice.addDirectParameterNameObserver(256, function (id, name) { params.names[String(id)] = str(name); });
    cursorDevice.addDirectParameterNormalizedValueObserver(function (id, value) { params.values[String(id)] = Number(value); });
    displayObserver = cursorDevice.addDirectParameterValueDisplayObserver(64, function (id, text) {
        params.displays[String(id)] = str(text);
    });

    // A Drum Machine's pads, when the selected device is one: pad n plays MIDI note n.
    drumPads = cursorDevice.createDrumPadBank(DRUM_PADS);
    drumPads.setIndication(false);
    drumPads.scrollPosition().markInterested();
    for (var p = 0; p < DRUM_PADS; p++) {
        var pad = drumPads.getItemAt(p);
        pad.exists().markInterested();
        pad.name().markInterested();
        var chain = pad.createDeviceBank(PAD_DEVICES);
        chain.itemCount().markInterested();
        var first = chain.getItemAt(0);
        first.exists().markInterested();
        first.name().markInterested();
        first.sampleName().markInterested();
        for (var d = 1; d < PAD_DEVICES; d++) {
            var after = chain.getItemAt(d);
            after.exists().markInterested();
            after.deviceType().markInterested();
        }
        padItems.push(pad);
        padDevices.push(chain);
        padFirst.push(first);
        padInserts.push(pad.insertionPoint());
        padReplace.push(first.replaceDeviceInsertionPoint());
    }

    tracks = host.createTrackBank(MAX_TRACKS, 0, SCENES, true);
    tracks.itemCount().markInterested();
    for (var t = 0; t < MAX_TRACKS; t++) {
        var track = tracks.getItemAt(t);
        track.exists().markInterested();
        track.name().markInterested();
        track.trackType().markInterested();
        track.isGroup().markInterested();
        track.color().markInterested();
        track.isActivated().markInterested();
        track.mute().markInterested();
        track.solo().markInterested();
        track.arm().markInterested();
        track.volume().markInterested();
        track.volume().displayedValue().markInterested();
        track.pan().markInterested();
        track.pan().displayedValue().markInterested();
        var selected = track.createEqualsValue(cursorTrack);
        selected.markInterested();
        trackSelected.push(selected);

        var devices = track.createDeviceBank(MAX_DEVICES);
        devices.itemCount().markInterested();
        for (var d = 0; d < MAX_DEVICES; d++) watchDevice(devices.getItemAt(d));
        trackDevices.push(devices);
        trackStarts.push(track.startOfDeviceChainInsertionPoint());
        trackEnds.push(track.endOfDeviceChainInsertionPoint());
        var tunings = [];
        for (var n = 0; n < TUNED_DEVICES; n++) {
            var device = devices.getItemAt(n);
            tunings.push({
                sampler: tuning(device, SAMPLER, "TRANSPOSE"),
                transpose: tuning(device, NOTE_TRANSPOSE, "COARSE")
            });
        }
        trackTunings.push(tunings);

        var slots = track.clipLauncherSlotBank();
        var slotList = [];
        var insertList = [];
        for (var s = 0; s < SCENES; s++) {
            var slot = slots.getItemAt(s);
            slot.exists().markInterested();
            slot.hasContent().markInterested();
            slotList.push(slot);
            insertList.push(slot.replaceInsertionPoint());
        }
        trackSlots.push(slotList);
        slotInserts.push(insertList);
    }

    popupBrowser = host.createPopupBrowser();
    popupBrowser.exists().markInterested();

    changesSetting = host.getPreferences().getBooleanSetting("Let the app make changes", "Music Organiser", false);
    changesSetting.markInterested();

    socket = host.createRemoteConnection("Music Organiser", PORT);
    socket.setClientConnectCallback(connected);
    port = socket.getPort();
    println("Music Organiser: waiting for the app on port " + (port > 0 ? port : PORT) + ".");
    warnIfPortTaken();
    // If Bitwig didn't say which port, ask again once it's had a moment (the extension's self-check does the same).
    host.scheduleTask(knownPort, PORT_CHECK_DELAY);
}

/**
 * The port Bitwig gave the link, asked again while Bitwig hasn't said: Bitwig 6.1.3 says -1 straight after
 * making the connection. The app connecting shows it's 19791, the only port the app tries. Warns in Bitwig
 * when it's another.
 */
function knownPort() {
    if (port <= 0 && socket) {
        port = socket.getPort();
        if (port <= 0 && clients.length > 0) port = PORT;
    }
    warnIfPortTaken();
    return port;
}

function warnIfPortTaken() {
    if (port <= 0 || port === PORT || portWarned) return;
    portWarned = true;
    host.showPopupNotification("Music Organiser: port " + PORT + " is taken, so the app can't connect. " +
                               "If Music Organiser is listed twice in Bitwig's Settings > Controllers, remove one; " +
                               "otherwise close what's using the port. Then restart Bitwig.");
}

function flush() {}

function exit() {
    for (var i = 0; i < clients.length; i++) clients[i].open = false;
    clients = [];
    jobs = [];
}

/** @param {API.Device} device */
function watchDevice(device) {
    device.exists().markInterested();
    device.name().markInterested();
    device.presetName().markInterested();
    device.isPlugin().markInterested();
    device.deviceType().markInterested();
    device.isEnabled().markInterested();
}

/**
 * One of a Bitwig device's parameters, for whenever `device` is that device (by the ID Bitwig saves it under).
 * @param {API.Device} device
 * @param {string} id
 * @param {string} param
 */
function tuning(device, id, param) {
    var parameter = device.createSpecificBitwigDevice(uuid(id)).createParameter(param);
    parameter.exists().markInterested();
    parameter.markInterested();
    parameter.displayedValue().markInterested();
    return parameter;
}

// ---- Talking to the app ----

/** @param {API.RemoteConnection} connection */
function connected(connection) {
    /** @type {Client} */
    var client = { connection: connection, open: true };
    clients.push(client);
    knownPort();
    connection.setDisconnectCallback(function () {
        client.open = false;
        remove(clients, client);
    });
    // data is a Java byte[] (the version 18 type definitions call it a number).
    connection.setReceiveCallback(function (/** @type {any} */ data) { handle(client, decodeUtf8(data)); });
    send(client, { event: "hello", result: hello() });
}

/**
 * The commands the app can send. Those marked `changes` only run when the setting allows it. A `job`
 * waits for Bitwig to make the change, so jobs run one at a time and answer when they're done.
 * @type {Object<string, { changes?: boolean, run?: function(Client, any, any): void, job?: function(any, Finish): void }>}
 */
var commands = {
    hello: { run: function (client, id) { reply(client, id, hello()); } },
    project: { run: function (client, id) { reply(client, id, describeProject()); } },
    device: { run: requestDevice },
    actions: { run: function (client, id) { reply(client, id, listActions()); } },
    select: { run: select },
    set: { changes: true, run: setParameter },
    insert: { changes: true, job: insert },
    newtrack: { changes: true, job: newTrack },
    clip: { changes: true, job: addClip },
    pads: { changes: true, job: fillPads },
    transpose: { changes: true, job: transpose },
    action: { changes: true, run: runAction }
};

/**
 * @param {Client} client
 * @param {string} text
 */
function handle(client, text) {
    var request;
    try {
        request = JSON.parse(text);
    } catch (e) {
        send(client, { id: null, ok: false, error: "The request isn't JSON: " + text.slice(0, 80) });
        return;
    }
    var id = request && request.id !== undefined ? request.id : null;
    try {
        if (!request || typeof request.cmd !== "string" || !has(commands, request.cmd))
            throw new LinkError("Unknown command: " + (request ? request.cmd : request));
        var command = commands[request.cmd];
        checkAllowed(request, !!command.changes);
        if (command.job) {
            jobs.push({ client: client, id: id, request: request, run: command.job, changes: !!command.changes });
            if (!currentJob) nextJob();
        } else {
            command.run(client, id, request);
        }
    } catch (e) {
        reply(client, id, null, failure(e));
    }
}

/**
 * Refuses a request that may not go ahead now. A request that carries `onlyIn` (a project's name) only goes
 * ahead while Bitwig has that project open, and a change only while "Let the app make changes" is on. This is
 * checked when a request arrives, when a change starts (it may have waited behind others) and before each step
 * of a change after a wait, since the track bank, the cursor track and the cursor device follow whichever
 * project is in front, and someone may bring their own to the front, or turn changes off, meanwhile.
 * @param {any} request
 * @param {boolean} changes whether it changes something
 */
function checkAllowed(request, changes) {
    var only = request.onlyIn;
    if (only !== undefined && only !== null) {
        if (typeof only !== "string") throw new LinkError("onlyIn has to be a project's name.");
        var open = str(app.projectName().get());
        if (open !== only)
            throw new LinkError("Bitwig has \"" + open + "\" open, not \"" + only + "\", so the link stopped before changing anything there.");
    }
    if (changes && !changesSetting.get()) throw new LinkError(CHANGES_OFF);
}

/** Runs the next change waiting, if any. */
function nextJob() {
    var job = jobs.shift();
    currentJob = job || null;
    if (!job) return;
    var finished = false;
    /** @type {Finish} */
    function finish(result, error) {
        if (finished) return;
        finished = true;
        reply(job.client, job.id, result, error);
        nextJob();
    }
    attempt(function () {
        checkAllowed(job.request, job.changes);
        job.run(job.request, finish);
    }, finish);
}

/**
 * Before a step of the change running now that comes after a wait: stops the change (with an error for
 * `attempt` to hand to its finish) unless it may still go ahead. Every step after a wait goes through
 * `waitFor` or `settle`, which call this, so nothing a change does after a wait can reach another project.
 * @param {Job} job the change the step belongs to
 */
function carryOn(job) {
    if (job) checkAllowed(job.request, job.changes);
}

/**
 * Runs one step of a change, ending the change with an error if the step fails.
 * @param {function(): void} step
 * @param {Finish} finish
 */
function attempt(step, finish) {
    try {
        step();
    } catch (e) {
        finish(null, failure(e));
    }
}

/** @param {any} e */
function failure(e) {
    return e instanceof LinkError ? e.message : "The Bitwig script failed: " + e;
}

/**
 * Bitwig makes changes after the script returns, and tells the script a moment later, so a change
 * waits: `test` is checked every 50 ms until it's true or `ms` have passed, and `then` gets whether it
 * came true. Each time, the change stops instead if it may no longer go ahead (see carryOn).
 * @param {function(): any} test
 * @param {number} ms
 * @param {function(boolean): void} then
 * @param {Finish} finish
 */
function waitFor(test, ms, then, finish) {
    var start = Date.now();
    var job = currentJob;
    function check() {
        if (job !== currentJob) return;   // a step left over from a change that has ended does nothing
        attempt(function () {
            carryOn(job);
            var ok = !!test();
            if (ok || Date.now() - start >= ms) then(ok);
            else host.scheduleTask(check, 50);
        }, finish);
    }
    host.scheduleTask(check, 50);
}

/**
 * Waits until what `look` sees stops changing (the same three times running), since Bitwig reports a
 * change bit by bit, then hands it to `then`. Each time, the change stops instead if it may no longer go
 * ahead (see carryOn).
 * @param {function(): any} look
 * @param {function(any): void} then
 * @param {Finish} finish
 */
function settle(look, then, finish) {
    var start = Date.now();
    var last = null;
    var same = 0;
    var job = currentJob;
    function check() {
        if (job !== currentJob) return;   // a step left over from a change that has ended does nothing
        attempt(function () {
            carryOn(job);
            var seen = look();
            var text = JSON.stringify(seen);
            same = text === last ? same + 1 : 0;
            last = text;
            if (same >= 2 || Date.now() - start >= CHANGE_WAIT) then(seen);
            else host.scheduleTask(check, 50);
        }, finish);
    }
    host.scheduleTask(check, 50);
}

/**
 * @param {Client} client
 * @param {any} id
 * @param {any} result
 * @param {string=} error
 */
function reply(client, id, result, error) {
    send(client, error ? { id: id, ok: false, error: error } : { id: id, ok: true, result: result });
}

/**
 * @param {Client} client
 * @param {Object} message
 */
function send(client, message) {
    if (!client.open) return;
    try {
        client.connection.send(/** @type {any} */ (asciiBytes(JSON.stringify(message) + "\n")));
    } catch (e) {
        client.open = false;
        remove(clients, client);
        errorln("Music Organiser: lost the connection to the app (" + e + ").");
    }
}

/**
 * An error to report to the app as it is.
 * @constructor
 * @param {string} message
 */
function LinkError(message) {
    this.message = message;
}

// ---- What the app can ask for ----

function hello() {
    return {
        link: LINK_VERSION,
        product: str(host.getHostProduct()),
        version: str(host.getHostVersion()),
        api: Number(host.getHostApiVersion()),
        platform: str(host.getPlatformType()),
        port: port,
        project: str(app.projectName().get()),
        modified: !!project.isModified().get(),
        engine: !!app.hasActiveEngine().get(),
        changes: !!changesSetting.get(),
        // This link honours a request's onlyIn (see checkAllowed).
        onlyIn: true
    };
}

function describeProject() {
    var list = [];
    for (var t = 0; t < MAX_TRACKS; t++) {
        var track = tracks.getItemAt(t);
        if (track.exists().get()) list.push(describeTrack(t, track, trackDevices[t]));
    }
    return {
        name: str(app.projectName().get()),
        modified: !!project.isModified().get(),
        tempo: Number(transport.tempo().getRaw()),
        timeSignature: str(transport.timeSignature().get()),
        playing: !!transport.isPlaying().get(),
        position: Number(transport.getPosition().get()),
        trackCount: Number(tracks.itemCount().get()),
        tracks: list
    };
}

/**
 * @param {number} index
 * @param {API.Track} track
 * @param {API.DeviceBank} devices
 */
function describeTrack(index, track, devices) {
    var list = [];
    for (var d = 0; d < MAX_DEVICES; d++) {
        var device = devices.getItemAt(d);
        if (device.exists().get()) list.push(describeDevice(d, device));
    }
    return {
        index: index,
        name: str(track.name().get()),
        type: str(track.trackType().get()),
        group: !!track.isGroup().get(),
        color: colour(track.color()),
        active: !!track.isActivated().get(),
        mute: !!track.mute().get(),
        solo: !!track.solo().get(),
        arm: !!track.arm().get(),
        volume: str(track.volume().displayedValue().get()),
        volumeValue: Number(track.volume().get()),
        pan: str(track.pan().displayedValue().get()),
        panValue: Number(track.pan().get()),
        deviceCount: Number(devices.itemCount().get()),
        devices: list
    };
}

/**
 * @param {number} index
 * @param {API.Device} device
 */
function describeDevice(index, device) {
    return {
        index: index,
        name: str(device.name().get()),
        preset: str(device.presetName().get()),
        kind: str(device.deviceType().get()),
        plugin: !!device.isPlugin().get(),
        enabled: !!device.isEnabled().get()
    };
}

/**
 * The selected device with all its parameters. Bitwig sends the display texts ("-6.0 dB") only once
 * they're asked for, so the reply waits for them (up to DEVICE_WAIT).
 * @param {Client} client
 * @param {any} id
 */
function requestDevice(client, id) {
    if (!cursorDevice.exists().get()) {
        reply(client, id, { track: str(cursorTrack.name().get()), device: null, params: [], drumPads: false });
        return;
    }
    deviceWaiters.push({ client: client, id: id });
    if (deviceWaiters.length > 1) return;   // already waiting: this request gets the same answer
    params.displays = {};
    displayObserver.setObservedParameterIds(params.ids.slice());
    deviceWaitStart = Date.now();
    host.scheduleTask(checkDevice, 50);
}

function checkDevice() {
    var missing = 0;
    for (var i = 0; i < params.ids.length; i++)
        if (!has(params.displays, params.ids[i])) missing++;
    if (missing > 0 && Date.now() - deviceWaitStart < DEVICE_WAIT) {
        host.scheduleTask(checkDevice, 50);
        return;
    }
    var result = describeSelectedDevice();
    displayObserver.setObservedParameterIds(null);
    var waiters = deviceWaiters;
    deviceWaiters = [];
    for (var w = 0; w < waiters.length; w++) reply(waiters[w].client, waiters[w].id, result);
}

function describeSelectedDevice() {
    var list = [];
    for (var i = 0; i < params.ids.length; i++) {
        var id = params.ids[i];
        list.push({
            id: id,
            name: has(params.names, id) ? params.names[id] : "",
            value: has(params.values, id) ? params.values[id] : null,   // NaN (not readable) becomes null in JSON
            display: has(params.displays, id) ? params.displays[id] : null
        });
    }
    var exists = !!cursorDevice.exists().get();
    return {
        track: str(cursorTrack.name().get()),
        device: exists ? describeDevice(-1, cursorDevice) : null,
        params: list,
        drumPads: exists && !!cursorDevice.hasDrumPads().get()
    };
}

function listActions() {
    var all = app.getActions();
    var list = [];
    for (var i = 0; i < all.length; i++) {
        var action = all[i];
        var category = action.getCategory();
        list.push({
            id: str(action.getId()),
            name: str(action.getName()),
            category: category ? str(category.getName()) : "",
            menu: str(action.getMenuItemText())
        });
    }
    return list;
}

/**
 * Selects a track from the project listing (by its index there) and, optionally, one of its devices.
 * @param {Client} client
 * @param {any} id
 * @param {{ track: number, device?: number }} request
 */
function select(client, id, request) {
    var t = request.track;
    checkTrack(t);
    var device = null;
    if (request.device !== undefined && request.device !== null) device = trackDevice(t, request.device);
    cursorTrack.selectChannel(tracks.getItemAt(t));
    if (device) cursorDevice.selectDevice(device);
    reply(client, id, {});
}

/**
 * Sets one of the selected device's parameters, by its ID, to a value from 0 to 1.
 * @param {Client} client
 * @param {any} id
 * @param {{ param: string, value: number }} request
 */
function setParameter(client, id, request) {
    if (!cursorDevice.exists().get()) throw new LinkError("No device is selected in Bitwig.");
    if (typeof request.param !== "string" || params.ids.indexOf(request.param) < 0)
        throw new LinkError("The selected device has no parameter " + request.param + ".");
    var value = request.value;
    if (typeof value !== "number" || !(value >= 0 && value <= 1))
        throw new LinkError("The value has to be a number from 0 to 1.");
    // Bitwig takes the value as a step out of (resolution - 1).
    cursorDevice.setDirectParameterValueNormalized(request.param, Math.round(value * 1000000), 1000001);
    reply(client, id, {});
}

/**
 * Runs one of Bitwig's actions by its ID (as listed by the actions command), e.g. saving the project. The
 * answer says which project Bitwig had in front just before the action ran, as `project` (the Java
 * extension says the same from 3.7), so the app can tell which project an action such as Save went to
 * when more than one is open. It also says the route the action ran by and that it ran (`via` and `ran`,
 * as the Java extension 3.7 does): the script only runs an action straight away ("direct"), so it
 * refuses a request for any other route, which only the Java extension has.
 * @param {Client} client
 * @param {any} id
 * @param {{ action: string, via?: any }} request
 */
function runAction(client, id, request) {
    if (request.via !== undefined && request.via !== null && request.via !== "direct")
        throw new LinkError("This needs the Java extension 3.7.");
    var action = typeof request.action === "string" ? app.getAction(request.action) : null;
    if (!action) throw new LinkError("Bitwig has no action " + request.action + ".");
    var front = str(app.projectName().get());
    action.invoke();
    reply(client, id, { project: front, via: "direct", ran: true });
}

// ---- Changes that wait for Bitwig ----

/**
 * Inserts something into a device chain: a plugin by its ID (as the app's plugin scanner reads it), a
 * Bitwig device by its ID, or a file (a preset, or a sample, which Bitwig loads into a Sampler). It goes
 * at the end of the selected track's chain, or of `track`'s (at its start with `at: "start"`). With a
 * track, the answer waits to say whether Bitwig added a device, since it quietly ignores what it can't use.
 * @param {{ kind: string, plugin?: any, device?: any, path?: any, track?: any, at?: any }} request
 * @param {Finish} finish
 */
function insert(request, finish) {
    var t = request.track;
    var start = request.at === "start";
    if (request.at !== undefined && request.at !== null && !start && request.at !== "end")
        throw new LinkError("Insert at the \"start\" or the \"end\" of the chain.");
    /** @type {API.InsertionPoint} */ var point;
    /** @type {API.DeviceBank} */ var chain = null;
    var before = 0;
    if (t === undefined || t === null) {
        if (start) throw new LinkError("Say which track's chain to insert at the start of.");
        if (!cursorTrack.exists().get()) throw new LinkError("No track is selected in Bitwig.");
        point = cursorTrack.endOfDeviceChainInsertionPoint();
    } else {
        checkTrack(t);
        chain = trackDevices[t];
        before = Number(chain.itemCount().get());
        point = start ? trackStarts[t] : trackEnds[t];
    }
    switch (request.kind) {
        case "vst3": point.insertVST3Device(given(request.plugin, "plugin ID")); break;
        case "clap": point.insertCLAPDevice(given(request.plugin, "plugin ID")); break;
        case "vst2":
            if (typeof request.plugin !== "number") throw new LinkError("A VST2 plugin's ID is a number.");
            point.insertVST2Device(request.plugin);
            break;
        case "bitwig": point.insertBitwigDevice(uuid(given(request.device, "device ID"))); break;
        case "file": point.insertFile(given(request.path, "file")); break;
        default: throw new LinkError("Can't insert a \"" + request.kind + "\": use vst3, clap, vst2, bitwig or file.");
    }
    if (!chain) {
        // Bitwig quietly does nothing for an ID it doesn't know; the app checks the device list afterwards.
        finish({});
        return;
    }
    var at = start ? 0 : before;
    waitFor(function () { return Number(chain.itemCount().get()) > before; }, CHANGE_WAIT, function (added) {
        if (!added) {
            finish({ added: false, index: null, device: null });
            return;
        }
        settle(function () {
            return at < MAX_DEVICES && chain.getItemAt(at).exists().get() ? describeDevice(at, chain.getItemAt(at)) : null;
        }, function (device) { finish({ added: true, index: at, device: device }); }, finish);
    }, finish);
}

/**
 * Adds an instrument track at the end of the project's tracks (before the effect tracks), names it and
 * selects it, and answers with its index in the project listing.
 * @param {{ name?: any, color?: any }} request
 * @param {Finish} finish
 */
function newTrack(request, finish) {
    var name = request.name === undefined || request.name === null ? "" : request.name;
    if (typeof name !== "string") throw new LinkError("A track's name is text.");
    var rgb = request.color === undefined || request.color === null ? null : parseColour(request.color);
    var count = Number(tracks.itemCount().get());
    var before = trackList();
    app.createInstrumentTrack(-1);
    // The new track shows in the listing where the tracks first differ: Bitwig adds it after the last
    // main track, so the effect tracks and the master move down one.
    waitFor(function () {
        return Number(tracks.itemCount().get()) > count && firstDifference(before, trackList()) >= 0;
    }, CHANGE_WAIT, function (shown) {
        if (!shown && Number(tracks.itemCount().get()) > count)
            throw new LinkError("Bitwig added the track past the first " + MAX_TRACKS + " tracks, where the app can't reach it.");
        if (!shown) throw new LinkError("Bitwig didn't add the track.");
        settle(trackList, function (after) {
            var t = firstDifference(before, after);
            if (t < 0 || !tracks.getItemAt(t).exists().get()) throw new LinkError("Bitwig didn't add the track.");
            var track = tracks.getItemAt(t);
            // In case adding a track opened Bitwig's browser to pick an instrument: the app brings its own.
            if (popupBrowser.exists().get()) popupBrowser.cancel();
            if (name.length > 0) track.name().set(name);
            if (rgb) track.color().set(rgb[0], rgb[1], rgb[2]);
            cursorTrack.selectChannel(track);
            waitFor(function () { return name.length === 0 || str(track.name().get()) === name; }, 1000, function () {
                finish({ index: t, name: str(track.name().get()) });
            }, finish);
        }, finish);
    }, finish);
}

/**
 * Puts a file (a MIDI file, or audio) in one of a track's clip launcher slots: the one given, or the
 * first empty one, and answers once Bitwig has made the clip.
 * @param {{ track: any, path: any, slot?: any }} request
 * @param {Finish} finish
 */
function addClip(request, finish) {
    var t = request.track;
    checkTrack(t);
    var path = given(request.path, "file");
    var slots = trackSlots[t];
    var s = request.slot;
    if (s !== undefined && s !== null) {
        if (!isIndex(s, SCENES)) throw new LinkError("The app can reach the first " + SCENES + " clip slots on a track, not slot " + s + ".");
        if (slots[s].hasContent().get()) throw new LinkError("Slot " + s + " on that track already has a clip.");
    } else {
        s = emptySlot(slots);
        if (s < 0) throw new LinkError("The track's first " + SCENES + " clip slots are full.");
    }
    var slot = slots[s];
    slotInserts[t][s].insertFile(path);
    waitFor(function () { return slot.hasContent().get(); }, CHANGE_WAIT, function (made) {
        if (!made) throw new LinkError("Bitwig didn't make a clip from " + path + ".");
        finish({ slot: s });
    }, finish);
}

/**
 * The first empty slot in a scene that exists, else the first of the scenes still to come; -1 if none.
 * @param {API.ClipLauncherSlot[]} slots
 */
function emptySlot(slots) {
    for (var s = 0; s < slots.length; s++)
        if (slots[s].exists().get() && !slots[s].hasContent().get()) return s;
    for (var n = 0; n < slots.length; n++)
        if (!slots[n].exists().get()) return n;
    return -1;
}

/**
 * Puts samples on a Drum Machine's free pads (as if each were dropped on its pad, so each gets a
 * Sampler), leaving pads that have something on them alone. A pad that holds only what a Drum Machine
 * preset put there, a Sampler with no sample, is free too: the sample goes in that Sampler's place,
 * keeping what comes after it. The Drum Machine is `device` on `track` (which this selects), or the
 * selected device.
 * @param {{ track?: any, device?: any, pads: any }} request
 * @param {Finish} finish
 */
function fillPads(request, finish) {
    var list = request.pads;
    if (!(list instanceof Array) || list.length === 0) throw new LinkError("No pads given.");
    var taken = {};
    for (var i = 0; i < list.length; i++) {
        var pad = list[i];
        if (!pad || !isIndex(pad.note, DRUM_PADS)) throw new LinkError("A pad's note is a MIDI note from 0 to 127.");
        if (has(taken, String(pad.note))) throw new LinkError("Two samples for note " + pad.note + ".");
        taken[String(pad.note)] = true;
        given(pad.path, "sample");
        if (pad.name !== undefined && pad.name !== null && typeof pad.name !== "string") throw new LinkError("A pad's name is text.");
    }
    var t = request.track;
    var d = request.device === undefined || request.device === null ? 0 : request.device;
    var onTrack = t !== undefined && t !== null;
    if (onTrack) {
        checkTrack(t);
        var device = trackDevice(t, d);
        cursorTrack.selectChannel(tracks.getItemAt(t));
        cursorDevice.selectDevice(device);
    } else if (!cursorDevice.exists().get()) {
        throw new LinkError("No device is selected in Bitwig. Select a Drum Machine and send the kit again.");
    }
    waitFor(function () {
        return cursorDevice.exists().get() && cursorDevice.hasDrumPads().get() &&
               (!onTrack || (trackSelected[t].get() && cursorDevice.position().get() === d));
    }, onTrack ? CHANGE_WAIT : 500, function (ready) {
        if (!ready)
            throw new LinkError(onTrack ? "Device " + d + " on track " + t + " isn't a Drum Machine."
                                        : "The selected device isn't a Drum Machine. Select one and send the kit again.");
        drumPads.scrollPosition().set(0);
        waitFor(function () { return drumPads.scrollPosition().get() === 0; }, 2000, function (scrolled) {
            if (!scrolled) throw new LinkError("Bitwig didn't show the Drum Machine's pads from the lowest note.");
            // The pads catch up with the selection a moment after it changes.
            settle(padList, function () { fill(list, finish); }, finish);
        }, finish);
    }, finish);
}

/**
 * Drops each sample on its free pad, or in place of the pad's empty Sampler, then waits for the pads
 * to take them and names them. The answer says which samples went in place of an empty Sampler, and
 * what's on each pad it leaves alone.
 * @param {{ note: number, path: string, name?: string }[]} list
 * @param {Finish} finish
 */
function fill(list, finish) {
    var filled = [];
    var replaced = {};   // note -> the pad's name before
    var skipped = [];
    var taken = [];
    for (var i = 0; i < list.length; i++) {
        var note = list[i].note;
        if (!padTaken(note)) {
            padInserts[note].insertFile(list[i].path);
        } else if (emptySampler(note)) {
            replaced[String(note)] = str(padItems[note].name().get());
            padReplace[note].insertFile(list[i].path);
        } else {
            skipped.push(note);
            taken.push(padContents(note));
            continue;
        }
        filled.push(list[i]);
    }
    /** @param {{ note: number }} pad */
    function took(pad) {
        var before = replaced[String(pad.note)];
        return before === undefined ? padTaken(pad.note)
            : str(padFirst[pad.note].sampleName().get()) !== "" || str(padItems[pad.note].name().get()) !== before;
    }
    waitFor(function () {
        for (var i = 0; i < filled.length; i++)
            if (!took(filled[i])) return false;
        return true;
    }, CHANGE_WAIT, function () {
        var done = [];
        var swapped = [];
        var failed = [];
        for (var i = 0; i < filled.length; i++) {
            var pad = filled[i];
            if (!took(pad)) {
                failed.push(pad.note);
                continue;
            }
            if (pad.name) padItems[pad.note].name().set(pad.name);
            done.push(pad.note);
            if (has(replaced, String(pad.note))) swapped.push(pad.note);
        }
        finish({ filled: done, replaced: swapped, skipped: skipped, failed: failed, taken: taken });
    }, finish);
}

/**
 * Whether a pad has a device on it: the pad, its device count and its first device all show it.
 * @param {number} note
 */
function padTaken(note) {
    return padItems[note].exists().get() && padDevices[note].itemCount().get() > 0 && padFirst[note].exists().get();
}

/**
 * Whether a pad holds a Sampler with no sample, as a Drum Machine preset can put on every pad
 * (Solomon's default Drum Machine does), still has the name it came with, not a sample's, and has
 * nothing after it but effects: such a pad is free.
 * @param {number} note
 */
function emptySampler(note) {
    var first = str(padFirst[note].name().get());
    var name = str(padItems[note].name().get());
    return padTaken(note) && first === "Sampler" && str(padFirst[note].sampleName().get()) === "" &&
           (name === "" || name === first) && onlyEffectsAfter(note);
}

/**
 * Whether all a pad has after its first device are audio effects, so nothing else on it makes a sound. A
 * pad with more devices than the link sees (PAD_DEVICES) doesn't count.
 * @param {number} note
 */
function onlyEffectsAfter(note) {
    var count = Number(padDevices[note].itemCount().get());
    if (count > PAD_DEVICES) return false;
    for (var d = 1; d < count; d++) {
        var after = padDevices[note].getItemAt(d);
        if (after.exists().get() && !audioEffect(str(after.deviceType().get()))) return false;
    }
    return true;
}

/**
 * Whether Bitwig's word for a device's type is an audio effect's. Bitwig 6.1.5 says "instrument" and
 * "note-effect" (the 2026-10-19 live run), so most likely "audio-effect"; "audio_to_audio", the word the link
 * was first written for, still counts.
 * @param {string} type
 */
function audioEffect(type) {
    return type === "audio-effect" || type === "audio_to_audio";
}

/**
 * What's on a pad: its name, how many devices, the first one's name and its sample, as Bitwig shows them.
 * @param {number} note
 */
function padContents(note) {
    return { note: note, name: str(padItems[note].name().get()), devices: Number(padDevices[note].itemCount().get()),
             first: str(padFirst[note].name().get()), sample: str(padFirst[note].sampleName().get()) };
}

/**
 * Sets the pitch of a Sampler (its Transpose, in semitones, which can be a fraction) or a Note Transpose
 * (whole semitones) among the first devices on a track, without selecting it, and answers with what
 * Bitwig shows.
 * @param {{ track: any, device: any, semitones: any }} request
 * @param {Finish} finish
 */
function transpose(request, finish) {
    var t = request.track;
    var d = request.device;
    var semitones = request.semitones;
    checkTrack(t);
    if (!isIndex(d, TUNED_DEVICES))
        throw new LinkError("The app can set the pitch of the first " + TUNED_DEVICES + " devices on a track, not device " + d + ".");
    var device = trackDevice(t, d);
    if (typeof semitones !== "number" || !isFinite(semitones)) throw new LinkError("Say how many semitones.");
    var tunings = trackTunings[t][d];
    // A device that was just added is known by its ID a moment after it appears.
    waitFor(function () { return tunings.sampler.exists().get() || tunings.transpose.exists().get(); }, 2000, function (known) {
        if (!known) throw new LinkError("Device " + d + " on track " + t + " is neither a Sampler nor a Note Transpose.");
        var param = tunings.sampler.exists().get() ? tunings.sampler : tunings.transpose;
        param.setRaw(semitones);
        waitFor(function () { return Math.abs(param.getRaw() - semitones) < 0.001; }, 1000, function () {
            finish({ device: str(device.name().get()), value: Number(param.getRaw()), display: str(param.displayedValue().get()) });
        }, finish);
    }, finish);
}

// ---- Helpers ----

/** @param {any} t */
function checkTrack(t) {
    if (!isIndex(t, MAX_TRACKS) || !tracks.getItemAt(t).exists().get())
        throw new LinkError("There's no track " + t + " in the project listing.");
}

/**
 * @param {number} t
 * @param {any} d
 */
function trackDevice(t, d) {
    if (!isIndex(d, MAX_DEVICES) || !trackDevices[t].getItemAt(d).exists().get())
        throw new LinkError("Track " + t + " has no device " + d + ".");
    return trackDevices[t].getItemAt(d);
}

/** Every track in the listing as "name|type" ("" where none), to tell which one is new. */
function trackList() {
    var list = [];
    for (var t = 0; t < MAX_TRACKS; t++) {
        var track = tracks.getItemAt(t);
        list.push(track.exists().get() ? str(track.name().get()) + "|" + str(track.trackType().get()) : "");
    }
    return list;
}

/**
 * Where the pad list starts, and what's on each pad (how many devices, less than 0 when the first is an
 * empty Sampler), to tell when it's caught up.
 */
function padList() {
    var list = [Number(drumPads.scrollPosition().get())];
    for (var p = 0; p < DRUM_PADS; p++) {
        var count = Number(padDevices[p].itemCount().get());
        list.push(!padTaken(p) ? 0 : emptySampler(p) ? -count : count);
    }
    return list;
}

/**
 * The first place two lists differ, or -1 if they don't.
 * @param {any[]} a
 * @param {any[]} b
 */
function firstDifference(a, b) {
    for (var i = 0; i < Math.max(a.length, b.length); i++)
        if (a[i] !== b[i]) return i;
    return -1;
}

/**
 * "#rrggbb" as red, green and blue from 0 to 1.
 * @param {any} text
 */
function parseColour(text) {
    var m = typeof text === "string" ? /^#([0-9a-fA-F]{2})([0-9a-fA-F]{2})([0-9a-fA-F]{2})$/.exec(text) : null;
    if (!m) throw new LinkError("A colour is written #rrggbb.");
    return [parseInt(m[1], 16) / 255, parseInt(m[2], 16) / 255, parseInt(m[3], 16) / 255];
}

/**
 * A java.util.UUID for insertBitwigDevice, where the script engine can make one.
 * @param {string} text
 * @returns {any}
 */
function uuid(text) {
    if (typeof Java !== "undefined" && Java.type) return Java.type("java.util.UUID").fromString(text);
    return text;
}

/**
 * @param {any} value
 * @param {number} count
 */
function isIndex(value, count) {
    return typeof value === "number" && value >= 0 && value < count && Math.floor(value) === value;
}

/**
 * @param {any} value
 * @param {string} what
 * @returns {string}
 */
function given(value, what) {
    if (typeof value !== "string" || value.length === 0) throw new LinkError("No " + what + " given.");
    return value;
}

/** @param {any} value */
function str(value) {
    return value === null || value === undefined ? "" : String(value);
}

/** @param {API.ColorValue} c */
function colour(c) {
    function hex(v) {
        var n = Math.max(0, Math.min(255, Math.round(Number(v) * 255)));
        return (n < 16 ? "0" : "") + n.toString(16);
    }
    return "#" + hex(c.red()) + hex(c.green()) + hex(c.blue());
}

/**
 * @param {Object} map
 * @param {string} key
 */
function has(map, key) {
    return Object.prototype.hasOwnProperty.call(map, key);
}

/**
 * The entries of `map` whose keys are in `keys`.
 * @param {Object<string, any>} map
 * @param {string[]} keys
 * @returns {Object<string, any>}
 */
function keep(map, keys) {
    var kept = {};
    for (var i = 0; i < keys.length; i++)
        if (has(map, keys[i])) kept[keys[i]] = map[keys[i]];
    return kept;
}

/**
 * @param {any[]} list
 * @param {any} item
 */
function remove(list, item) {
    var i = list.indexOf(item);
    if (i >= 0) list.splice(i, 1);
}

/**
 * A request's bytes as text. Bitwig hands them over as signed bytes.
 * @param {ArrayLike<number>} bytes
 */
function decodeUtf8(bytes) {
    var out = [];
    for (var i = 0; i < bytes.length;) {
        var c = bytes[i++] & 0xff;
        if (c < 0x80) {
            out.push(c);
        } else if (c < 0xe0) {
            out.push(((c & 0x1f) << 6) | (bytes[i++] & 0x3f));
        } else if (c < 0xf0) {
            out.push(((c & 0x0f) << 12) | ((bytes[i++] & 0x3f) << 6) | (bytes[i++] & 0x3f));
        } else {
            var cp = (((c & 0x07) << 18) | ((bytes[i++] & 0x3f) << 12) | ((bytes[i++] & 0x3f) << 6) | (bytes[i++] & 0x3f)) - 0x10000;
            out.push(0xd800 + (cp >> 10), 0xdc00 + (cp & 0x3ff));
        }
    }
    var text = "";
    for (var j = 0; j < out.length; j += 4096) text += String.fromCharCode.apply(null, out.slice(j, j + 4096));
    return text;
}

/**
 * JSON as bytes for Bitwig to send. Anything outside ASCII is written as a \u escape (it can only be
 * inside a JSON string), so every character is one byte.
 * @param {string} json
 */
function asciiBytes(json) {
    var escaped = json.replace(/[\u007f-\uffff]/g, function (c) {
        return "\\u" + ("000" + c.charCodeAt(0).toString(16)).slice(-4);
    });
    var bytes = new Array(escaped.length);
    for (var i = 0; i < escaped.length; i++) bytes[i] = escaped.charCodeAt(i);
    return bytes;
}
