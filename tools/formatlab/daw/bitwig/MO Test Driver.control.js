// Music Organiser test driver for Bitwig Studio (Format Lab).
//
// Records known automation into the live test project and saves it, so Format Lab can read how Bitwig stores each
// kind of automation lane. It only ever works in a project named exactly "MO Live Test automation" (rule 10), and only
// while "Let the driver make changes" is on in this controller's settings.
//
// Installing: copy this file into Bitwig's "Controller Scripts\MO Test Driver" folder (in Documents\Bitwig Studio),
// then in Bitwig: Settings > Controllers > Add controller > Music Organiser > Test driver. Then open the live test
// project, turn on "Let the driver make changes" and press "Record automation test" in the same settings.
//
// What it records, from the start of the arrangement, with arranger automation write on:
//   the tempo, the master track's volume, a new instrument track's volume and pan, and its MO Probe's on/off,
//   each set to the values in STEPS, one step every two seconds, each control at its own moment in the step and
//   touched only while it's set (Bitwig 6.1.6 missed some volume and pan moves made all at once); then it stops,
//   turns automation write off and saves.
// The values are written to Bitwig's Controller Script console as they're set, so the saved file can be checked
// against them.
//
// Plain ES5 and plain ASCII (rule 11).

loadAPI(18);
host.defineController("Music Organiser", "Test driver", "1.0", "6f0f5d38-7c1e-4b8e-9d0a-3a8f1c2b7e41", "Music Organiser");
host.defineMidiPorts(0, 0);

var PROJECT = "MO Live Test automation";
/** MO Probe (the old app's test plugin, installed on Solomon's PC): its report logs each tempo change and whether
 * Bitwig has it switched on, with positions, a check on what Bitwig recorded beside the saved file. */
var PROBE = "org.musicorganiser.probe.instrument";
/** Milliseconds between steps, and before the first. */
var STEP_MS = 2000;
/** Milliseconds between one control's move and the next within a step, and how long each is touched. */
var CONTROL_MS = 300;
var TOUCH_MS = 250;
/**
 * The values set at each step: tempo in BPM, the others 0..1 as Bitwig's API takes them (pan 0.5 is the middle),
 * and whether the MO Probe is on.
 */
var STEPS = [
    { tempo: 90, master: 0.6, volume: 0.4, pan: 0.25, on: false },
    { tempo: 140, master: 0.9, volume: 0.8, pan: 0.75, on: true },
    { tempo: 120, master: 0.75, volume: 0.6, pan: 0.5, on: false },
    { tempo: 100, master: 0.5, volume: 0.7, pan: 0.0, on: true }
];

var app, project, popupBrowser, transport, master, cursorTrack, cursorDevice, changes;
var running = false;
/** Set when Save should run at Bitwig's next flush (Bitwig 6.1.3 only saved for a script from there). */
var saveAtFlush = false;
/** The automation write mode before the run, put back after it. */
var writeModeBefore = null;

function init() {
    app = host.createApplication();
    app.projectName().markInterested();
    project = host.getProject();
    project.isModified().markInterested();
    // Adding a track can open Bitwig's browser, which then holds the selection: it's closed (as the link does).
    popupBrowser = host.createPopupBrowser();
    popupBrowser.exists().markInterested();
    transport = host.createTransport();
    transport.tempo().markInterested();
    transport.isPlaying().markInterested();
    transport.getPosition().markInterested();
    transport.isArrangerAutomationWriteEnabled().markInterested();
    transport.automationWriteMode().markInterested();
    master = host.createMasterTrack(0);
    master.volume().markInterested();
    cursorTrack = host.createCursorTrack("MO_TEST", "MO test driver", 0, 0, true);
    cursorTrack.exists().markInterested();
    cursorTrack.name().markInterested();
    cursorTrack.volume().markInterested();
    cursorTrack.pan().markInterested();
    cursorDevice = cursorTrack.createCursorDevice();
    cursorDevice.exists().markInterested();
    cursorDevice.name().markInterested();
    cursorDevice.isEnabled().markInterested();

    var prefs = host.getPreferences();
    changes = prefs.getBooleanSetting("Let the driver make changes", "Music Organiser test driver", false);
    changes.markInterested();
    prefs.getSignalSetting("Record automation test", "Music Organiser test driver", "Record")
        .addSignalObserver(start);
    println("MO test driver ready. It only works in \"" + PROJECT + "\".");
}

function say(text) {
    println("MO test driver: " + text);
    host.showPopupNotification("MO test driver: " + text);
}

/** Whether it may go on: the live test project is in front and changes are on. */
function allowed() {
    var open = String(app.projectName().get());
    if (open !== PROJECT) {
        say("\"" + open + "\" is open, not \"" + PROJECT + "\", so nothing was changed.");
        return false;
    }
    if (!changes.get()) {
        say("turn on \"Let the driver make changes\" first.");
        return false;
    }
    return true;
}

function start() {
    if (running || !allowed()) return;
    running = true;
    say("adding an instrument track with a MO Probe");
    app.createInstrumentTrack(-1);
    host.scheduleTask(function () {
        if (!allowed()) return finish();
        if (popupBrowser.exists().get()) popupBrowser.cancel();
        cursorTrack.endOfDeviceChainInsertionPoint().insertCLAPDevice(PROBE);
        host.scheduleTask(record, 1500);
    }, 1500);
}

function record() {
    if (!allowed()) return finish();
    if (popupBrowser.exists().get()) popupBrowser.cancel();
    if (!cursorDevice.exists().get() || String(cursorDevice.name().get()).indexOf("MO Probe") < 0) {
        say("the selected device isn't the new MO Probe, so nothing was recorded.");
        return finish();
    }
    say("recording on \"" + cursorTrack.name().get() + "\", device \"" + cursorDevice.name().get() + "\"");
    writeModeBefore = String(transport.automationWriteMode().get());
    transport.automationWriteMode().set("write");
    transport.isArrangerAutomationWriteEnabled().set(true);
    transport.getPosition().set(0);
    transport.play();
    for (var i = 0; i < STEPS.length; i++) host.scheduleTask(step(i), STEP_MS * (i + 1));
    host.scheduleTask(stopAndSave, STEP_MS * (STEPS.length + 1));
}

/** Sets one step's values, one control at a time, each touched while it's set so Bitwig writes it. */
function step(i) {
    return function () {
        if (!allowed()) return;
        var s = STEPS[i];
        var moves = [
            function () { touchSet(transport.tempo(), function (p) { p.setRaw(s.tempo); }); },
            function () { touchSet(master.volume(), function (p) { p.set(s.master); }); },
            function () { touchSet(cursorTrack.volume(), function (p) { p.set(s.volume); }); },
            function () { touchSet(cursorTrack.pan(), function (p) { p.set(s.pan); }); },
            function () { cursorDevice.isEnabled().set(s.on); }
        ];
        for (var m = 0; m < moves.length; m++) host.scheduleTask(move(moves[m]), CONTROL_MS * m);
        println("MO test driver: step " + (i + 1) + " from beat " + Number(transport.getPosition().get()).toFixed(3) +
                ": tempo " + s.tempo + ", master " + s.master + ", volume " + s.volume + ", pan " + s.pan +
                ", probe " + (s.on ? "on" : "off"));
    };
}

/** A move that's skipped once the live test project isn't in front any more (rule 10). */
function move(set) {
    return function () {
        if (running && allowed()) set();
    };
}

function touchSet(param, set) {
    param.touch(true);
    set(param);
    host.scheduleTask(function () { param.touch(false); }, TOUCH_MS);
}

function stopAndSave() {
    // Another project brought to the front meanwhile is left alone (rule 10).
    if (!allowed()) return finish();
    transport.stop();
    transport.isArrangerAutomationWriteEnabled().set(false);
    if (writeModeBefore) transport.automationWriteMode().set(writeModeBefore);
    host.scheduleTask(function () {
        if (!allowed()) return finish();
        saveAtFlush = true;
        host.requestFlush();
        // Bitwig may not save for a script with no MIDI ports: then it's said, so Ctrl+S saves it by hand.
        host.scheduleTask(function () {
            if (project.isModified().get())
                say("recorded " + STEPS.length + " steps, but Bitwig didn't save: press Ctrl+S to save it.");
            else
                say("recorded " + STEPS.length + " steps and saved \"" + PROJECT + "\".");
            finish();
        }, 2000);
    }, 1000);
}

function flush() {
    if (!saveAtFlush) return;
    saveAtFlush = false;
    if (allowed()) app.getAction("Save").invoke();
}

function finish() {
    running = false;
}

function exit() {
}
