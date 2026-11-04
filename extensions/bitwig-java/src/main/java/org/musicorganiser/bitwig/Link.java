package org.musicorganiser.bitwig;

import com.bitwig.extension.controller.api.Action;
import com.bitwig.extension.controller.api.ActionCategory;
import com.bitwig.extension.controller.api.Application;
import com.bitwig.extension.controller.api.Arranger;
import com.bitwig.extension.controller.api.BooleanValue;
import com.bitwig.extension.controller.api.Clip;
import com.bitwig.extension.controller.api.ClipLauncherSlot;
import com.bitwig.extension.controller.api.ClipLauncherSlotBank;
import com.bitwig.extension.controller.api.ControllerHost;
import com.bitwig.extension.controller.api.CursorRemoteControlsPage;
import com.bitwig.extension.controller.api.CursorTrack;
import com.bitwig.extension.controller.api.Device;
import com.bitwig.extension.controller.api.DeviceBank;
import com.bitwig.extension.controller.api.DirectParameterValueDisplayObserver;
import com.bitwig.extension.controller.api.DrumPad;
import com.bitwig.extension.controller.api.DrumPadBank;
import com.bitwig.extension.controller.api.HardwareButton;
import com.bitwig.extension.controller.api.HardwareSurface;
import com.bitwig.extension.controller.api.InsertionPoint;
import com.bitwig.extension.controller.api.LastClickedParameter;
import com.bitwig.extension.controller.api.MasterRecorder;
import com.bitwig.extension.controller.api.MasterTrack;
import com.bitwig.extension.controller.api.MidiIn;
import com.bitwig.extension.controller.api.MidiOut;
import com.bitwig.extension.controller.api.Parameter;
import com.bitwig.extension.controller.api.PinnableCursorDevice;
import com.bitwig.extension.controller.api.PlayingNote;
import com.bitwig.extension.controller.api.PopupBrowser;
import com.bitwig.extension.controller.api.Project;
import com.bitwig.extension.controller.api.RemoteConnection;
import com.bitwig.extension.controller.api.RemoteSocket;
import com.bitwig.extension.controller.api.SettableBooleanValue;
import com.bitwig.extension.controller.api.SettableColorValue;
import com.bitwig.extension.controller.api.SettableEnumValue;
import com.bitwig.extension.controller.api.Track;
import com.bitwig.extension.controller.api.TrackBank;
import com.bitwig.extension.controller.api.Transport;
import java.io.PrintWriter;
import java.io.StringWriter;
import java.net.URI;
import java.net.URISyntaxException;
import java.net.URL;
import java.nio.file.Files;
import java.nio.file.Path;
import java.security.CodeSource;
import java.time.Clock;
import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Comparator;
import java.util.Deque;
import java.util.HashMap;
import java.util.HashSet;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.Objects;
import java.util.Set;
import java.util.UUID;
import java.util.function.BooleanSupplier;
import java.util.function.Consumer;
import java.util.function.IntConsumer;
import java.util.function.Predicate;
import java.util.function.Supplier;
import java.util.regex.Matcher;
import java.util.regex.Pattern;
import java.util.stream.Collectors;

/**
 * The Music Organiser link for Bitwig Studio.
 *
 * Lets the Music Organiser app see what's in the open Bitwig project: its tracks and devices, the selected
 * device's parameters (with the IDs Bitwig saves them under) and Bitwig's actions. With "Let the app make
 * changes" turned on in this controller's settings, the app can also add tracks, set a parameter, insert a
 * plugin, device, preset or sample, put a MIDI file in a clip slot, fill a Drum Machine's pads, run an action
 * such as Save, start and stop Bitwig's master recorder, and (an experiment) copy a launcher clip into the
 * arranger. That's how the app sends an FL Studio channel or a kit into Bitwig, and how it learns Bitwig's file
 * format: change one thing, save, and compare.
 *
 * The app connects to port 19791 on this computer and sends JSON requests, one per message (see {@link
 * Framing}). The commands are described in extensions/README.md. This is a port of the JavaScript controller
 * script (extensions/bitwig/MusicOrganiser.control.js), function by function, so the app talks to either the
 * same way; it adds what only a Java extension can do (newer API, and reports written to files: see {@link
 * Reports}).
 *
 * Everything here runs on Bitwig's controller thread: in init, in Bitwig's callbacks and in tasks given to
 * host.scheduleTask. Bitwig's objects can only be made in init, and values reach the extension a moment after
 * Bitwig changes them, so every value read later is marked as interesting in init, and a change waits for
 * Bitwig to report it.
 *
 * Bitwig 6.1.3 ignored every command of its user interface the link ran (Save, copy, paste, bringing a panel to the
 * front) while it carried out every other change, whether the link ran them as a request came in or in a task it
 * scheduled. The same Save works for controllers such as DrivenByMoss, which run it while Bitwig handles a hardware
 * button. So the link can run such a command by a route ("via"): straight away, from Bitwig's flush, or through a MIDI
 * loopback, as a message it sent itself or a press of a button of its own coming back (see {@link #ROUTES}). Save
 * worked from Bitwig's flush (live run 20261015T185248Z), so that's the route a request gets when it doesn't name one
 * (see {@link #DEFAULT_ROUTE}); copy and paste still didn't put a clip in the arranger by any route.
 *
 * That's because Bitwig copies and pastes whatever was clicked last in it, not what the API selected (live runs of 3.6 to
 * 3.8). So from extension 3.9 the link records a launcher clip into the arranger instead, as a person does with Bitwig's
 * arranger record on, which takes only ordinary calls of the API (see {@link #record}), and finds tracks by the IDs Bitwig
 * keeps them under, which stay put when a track goes in above them.
 */
final class Link {
    static final int PORT = 19791;
    /** The protocol's version: 3 adds the newer API's fields and the clicked, recorder and selftest commands. */
    static final int LINK_VERSION = 3;
    /** Tracks the project listing covers (a flat list: tracks inside groups, effect tracks and the master). */
    static final int MAX_TRACKS = 128;
    /** Devices listed per track: the top level of its device chain. */
    static final int MAX_DEVICES = 16;
    /** Clip launcher slots per track the app can fill: those of the first scenes. */
    static final int SCENES = 8;
    /** A Drum Machine's pads: one for each MIDI note. */
    static final int DRUM_PADS = 128;
    /** How many of each pad's devices the link sees: enough to tell an empty Sampler with effects after it from a pad that plays. */
    static final int PAD_DEVICES = 4;
    /** Devices at the start of each track whose pitch the app can set (a Sampler or a Note Transpose). */
    static final int TUNED_DEVICES = 2;
    /** The selected device's remote controls the link reads steps, origins and automation from (one page). */
    static final int CONTROLS = 8;
    /** How long to wait for the selected device's parameter values to arrive, in milliseconds. */
    static final long DEVICE_WAIT = 2000;
    /** How long to wait for Bitwig to make a change the app asked for (a track, a device, a clip). */
    static final long CHANGE_WAIT = 5000;
    /**
     * How long a replaced clip gets to be swapped (its name changing) before it's launched anyway (extension 3.16; from
     * 3.17 it's launched as soon as the new one is seen).
     */
    static final long REPLACE_WAIT = 2000;
    /** When to write the first self-check: once Bitwig has sent the values the link watches. */
    static final long SELF_CHECK_DELAY = 1000;
    /** Bitwig's own devices the app tunes, by the IDs Bitwig saves them under. */
    static final String SAMPLER = "468bc14b-b2e7-45a1-9666-e83117fe404e";
    static final String NOTE_TRANSPOSE = "0815cd9e-3a31-4429-a268-dabd952a3b68";
    /** Bitwig's action that brings the arranger timeline to the front, or hides it when it's in front already. */
    static final String FOCUS_ARRANGER = "focus_or_toggle_arranger";
    /**
     * The same for the clip launcher, which arrange runs before it copies when Bitwig has an action by this ID: Bitwig
     * copies from whichever panel has the focus. Nobody has seen it in Bitwig's list of actions yet, so arrange does
     * without it when it isn't there.
     */
    static final String FOCUS_LAUNCHER = "focus_or_toggle_clip_launcher";
    /** How long to wait to see whether such an action hid its panel, rather than bringing it to the front. */
    static final long TOGGLE_WAIT = 500;
    /**
     * The golden ratio's fraction. Stepping round the colour wheel by it, each colour arrange marks a launcher clip with
     * lands far from the one before (and from the one before that).
     */
    static final double GOLDEN = 0.6180339887498949;
    /** How far apart two colours' red, green and blue may be (out of 255 each) and still count as the same colour. */
    static final int SAME_COLOUR = 2;
    /** How far apart one of them has to be (out of 255) for a colour to look clearly different. */
    static final int OTHER_COLOUR = 64;
    /** How often the record job (extension 3.9) looks at what Bitwig is doing, in milliseconds. */
    static final long RECORD_TICK = 25;
    /** How long the record job gives Bitwig to start playing once it has launched the clip, in milliseconds. */
    static final long START_WAIT = 3000;
    /**
     * How long Bitwig has to show stopped, after the record job last pressed Stop or saw it play, before the job counts it
     * stopped for sure (what Bitwig shows reaches the extension a moment late), in milliseconds.
     */
    static final long SETTLE = 500;
    /** How long the record job waits after pressing Stop before pressing it again while Bitwig still shows playing. */
    static final long RESTOP = 500;
    /**
     * How long past the clip's end the record job lets the clip's track go on playing before it ends the recording itself
     * (see watch), in milliseconds: Bitwig shows the track stopping a moment late.
     */
    static final long LATE_STOP = 150;
    /** The most lines a record answer's trace keeps (a line a look, 25 ms apart, and one for each command it sends). */
    static final int MAX_TRACE = 1200;
    /** The tempos Bitwig takes, in BPM: the record job asks for none outside them. */
    static final double MIN_TEMPO = 20;
    static final double MAX_TEMPO = 666;
    /** The most notes a record answer lists for each time the clip plays (it counts them all). */
    static final int MAX_NOTES = 64;
    /** How long the probe (extension 3.9) waits after each of Bitwig's commands before it looks, unless asked for another gap. */
    static final long PROBE_GAP = 1500;
    /** Bitwig's action that shows or hides the arranger timeline, whichever panel has the focus. */
    static final String TOGGLE_ARRANGER = "toggle_arranger";
    /**
     * The routes the link can run one of Bitwig's commands by (extension 3.7): "direct" runs it straight away (in the
     * socket's callback, or a task the link scheduled), "flush" from Bitwig's next flush, "midi" when a SysEx message the
     * link sent itself comes back through the loopback, "button" by pressing one of the link's own hardware buttons through
     * the loopback, which Bitwig runs the command for itself, and "buttonCode" by pressing another, whose press the link
     * runs the command for. The last three need the controller's MIDI in and out set to the loopMIDI port called
     * "Music Organiser Bitwig". Bitwig 6.1.3 has only carried out Save for the link once they were set (from its flush both
     * times, straight away once in two); without them only "direct" was tried, and it didn't save.
     */
    static final List<String> ROUTES = List.of("direct", "flush", "midi", "button", "buttonCode");
    static final String DIRECT = "direct";
    /**
     * The route a request gets when it doesn't name one (extension 3.8; "direct" before): Bitwig's flush, the route Bitwig
     * 6.1.3 carried out Save by (in 139 ms), where straight away it left the project unsaved.
     */
    static final String DEFAULT_ROUTE = "flush";
    /**
     * The commands the link has buttons for, in order: Bitwig's Save, Copy and Paste actions, the two focus actions, and
     * (extension 3.8) Duplicate, which arrange's checks run.
     */
    static final List<String> ROUTED = List.of("Save", "Copy", "Paste", FOCUS_ARRANGER, FOCUS_LAUNCHER, "Duplicate");
    /** The MIDI channel the link's buttons listen on: 16 (Bitwig counts from 0). */
    static final int CHANNEL = 15;
    /**
     * The notes of the link's buttons, the nth command of ROUTED on the nth note up from each: from NOTE_BOUND for the
     * "button" route (Bitwig runs the command bound to it), from NOTE_CODE for "buttonCode" (the link runs it).
     */
    static final int NOTE_BOUND = 100;
    static final int NOTE_CODE = 110;
    /** How long a route other than "direct" has to come back (through Bitwig's flush, or the loopback) before the link gives up on it. */
    static final long LOOPBACK_WAIT = 1000;
    /**
     * The start of every SysEx message the link sends itself: F0, then 7D (the ID for non-commercial use, so no maker's
     * device takes it for its own), then "MOB" (4D 4F 42). Three bytes of the message's number follow, then F7.
     */
    static final String SYSEX_START = "F07D4D4F42";
    /** How to set up the loopback, for the self-check and the answers that need it. */
    static final String LOOPBACK_SETUP = "In Bitwig, Settings > Controllers > Music Organiser, set both MIDI in and MIDI out to \""
                                         + MusicOrganiserExtensionDefinition.LOOPBACK_PORT + "\" (the loopMIDI port), never \"Music Organiser\", "
                                         + "which is the FL Studio link's.";
    static final String CHANGES_OFF = "Changes are turned off. To let the app change things, turn on \"Let the app make changes\" "
                                      + "in Bitwig: Settings > Controllers > Music Organiser.";
    /** Why a change that waited behind others wasn't made: the app that asked had closed its connection (extension 3.18). */
    static final String APP_GONE = "The app stopped waiting for this change, so the link didn't make it.";
    /**
     * The note grid `notes` reads a clip through: steps of a 16th note (in beats), 1024 of them (256 beats, 64 bars of 4/4),
     * on all 128 keys. A note that starts past the grid isn't read.
     */
    static final double NOTE_STEP = 0.25;
    static final int NOTE_STEPS = 1024;
    static final int NOTE_KEYS = 128;
    static final String NO_CLIP = "No clip is selected in Bitwig. Click your chord clip in the arranger or the clip launcher, then try again.";

    private static final Pattern COLOUR = Pattern.compile("^#([0-9a-fA-F]{2})([0-9a-fA-F]{2})([0-9a-fA-F]{2})$");

    private final ControllerHost host;
    private final Reports reports;
    private final Clock clock;

    private Application app;
    private Project project;
    private Transport transport;
    /** Whether the transport's play position and play start could be followed (extension 3.16). */
    private boolean followed;
    private TrackBank tracks;
    private final Track[] trackItems = new Track[MAX_TRACKS];
    private final DeviceBank[] trackDevices = new DeviceBank[MAX_TRACKS];
    /** Where things go on each track: the start and end of its device chain. */
    private final InsertionPoint[] trackStarts = new InsertionPoint[MAX_TRACKS];
    private final InsertionPoint[] trackEnds = new InsertionPoint[MAX_TRACKS];
    /** Each track's clip launcher slots, and what replaces their contents. */
    private final ClipLauncherSlot[][] trackSlots = new ClipLauncherSlot[MAX_TRACKS][];
    private final InsertionPoint[][] slotInserts = new InsertionPoint[MAX_TRACKS][];
    /** The pitch of the first devices on each track, for when they're a Sampler or a Note Transpose. */
    private final Tuning[][] trackTunings = new Tuning[MAX_TRACKS][];
    /** Whether each track is the selected one. */
    private final BooleanValue[] trackSelected = new BooleanValue[MAX_TRACKS];
    private CursorTrack cursorTrack;
    private InsertionPoint cursorEnd;
    private PinnableCursorDevice cursorDevice;
    private DirectParameterValueDisplayObserver displayObserver;
    /** The selected device's drum pads, when it's a Drum Machine. */
    private DrumPadBank drumPads;
    private final DrumPad[] padItems = new DrumPad[DRUM_PADS];
    /** Each pad's devices, the first of them, and where to put a sample in place of that one. */
    private final DeviceBank[] padDevices = new DeviceBank[DRUM_PADS];
    private final Device[] padFirst = new Device[DRUM_PADS];
    private final InsertionPoint[] padInserts = new InsertionPoint[DRUM_PADS];
    private final InsertionPoint[] padReplace = new InsertionPoint[DRUM_PADS];
    private PopupBrowser popupBrowser;
    private SettableBooleanValue changesSetting;
    private RemoteSocket socket;
    /** The port Bitwig gave the link; 0 or less while Bitwig hasn't said (see {@link #knownPort}). */
    private int port = PORT;
    private boolean listening;
    /** Whether Bitwig has been told the port is taken (once is enough). */
    private boolean portWarned;
    /** What most often takes the port: the link itself, loaded twice. */
    static final String TWICE = "If Music Organiser is listed twice in Bitwig's Settings > Controllers, remove one; "
                                + "otherwise close what's using the port. Then restart Bitwig.";

    // Newer API (version 20 to 23), each null when Bitwig wouldn't make it.
    private CursorRemoteControlsPage remoteControls;
    private Parameter[] controls;
    private LastClickedParameter lastClicked;
    private Parameter clickedParameter;
    private MasterRecorder recorder;
    /** Bitwig's arranger, for the arrange experiment: whether its timeline shows, and the clip selected there, on its track. */
    private Arranger arranger;
    private Clip arrangerClip;
    private Track arrangerClipTrack;
    /**
     * The clips selected in the arranger and in the clip launcher (in that order), each with the notes its grid shows, for
     * `notes` (null when Bitwig wouldn't make them).
     */
    private List<NoteClip> noteClips;
    /** Where a file dropped after each track goes, for the filetrack experiment (null when Bitwig wouldn't make them). */
    private InsertionPoint[] trackAfter;
    /**
     * The hue (0 to 1, round the colour wheel) of the colour arrange marked a launcher clip with last, so the next mark
     * differs from it. It starts from the clock, once, so the marks differ from one time Bitwig starts the link to the
     * next as well. The tests' stand-in Bitwig sets it, to know which colours come.
     */
    double markHue = Math.floorMod(System.nanoTime(), 1_000_000L) / 1_000_000.0;
    /** The loopback's MIDI ports (null when Bitwig wouldn't give them), the link's buttons, and the commands they're for. */
    private MidiIn midiIn;
    private MidiOut midiOut;
    private HardwareSurface surface;
    /** The commands of ROUTED Bitwig has, each with its place in ROUTED (which sets its buttons' notes). */
    private Map<String, Integer> buttons;
    /** Whether any of the link's own loopback messages (a SysEx or a button press) has come back since it started. */
    private boolean loopback;
    /** The number of the last SysEx message the link sent itself (1 up). */
    private int lastToken;
    /** Commands on their way by a route other than "direct": by their message's number, by button, and for the next flush. */
    private final Map<Integer, Sent> tokens = new HashMap<>();
    private final Map<String, Sent> presses = new HashMap<>();
    private final List<Sent> flushing = new ArrayList<>();
    /** Whether parameters carry their steps, origin and automation (marked as interesting in init). */
    private boolean details;
    /** Whether tracks carry their IDs. */
    private boolean trackIds;
    /** Bitwig's master track, which the record job mutes while it plays (null when Bitwig wouldn't make it, or the rest of recording). */
    private MasterTrack master;
    /** The record job running now, if any: recordabort and Bitwig stopping the link reach it here. */
    private Recording recording;

    /**
     * The selected device's parameters, as Bitwig reports them. Display texts are only asked for while a request
     * waits for them, since Bitwig advises against watching them all the time.
     */
    private final Params params = new Params();

    private final List<Client> clients = new ArrayList<>();
    /** Requests waiting for the selected device's parameter values. */
    private final List<Ask> deviceWaiters = new ArrayList<>();
    private long deviceWaitStart;

    /** Changes that wait for Bitwig, run one at a time in the order they were asked for. */
    private final Deque<Job> jobs = new ArrayDeque<>();
    /** The change running now, if any: each of its steps after a wait checks it may still go ahead. */
    private Job currentJob;

    /** What init made of each part of Bitwig's API, for the self-check. */
    private final List<Feature> features = new ArrayList<>();

    /**
     * The commands the app can send. Those marked `changes` only run when the setting allows it. A job waits
     * for Bitwig to make the change, so jobs run one at a time and answer when they're done.
     */
    private final Map<String, Command> commands = new HashMap<>();

    /**
     * The file Bitwig loaded the extension from, or "" when that can't be told. Bitwig may load extensions from another
     * Bitwig Studio folder than the one holding the projects (Documents, while the projects are on another drive), so
     * the app needs this to put a new version where Bitwig looks.
     */
    String loadedFrom = loadedFrom(Link.class);

    Link(ControllerHost host, Reports reports, Clock clock) {
        this.host = host;
        this.reports = reports;
        this.clock = clock;
        commands.put("hello", Command.answer(false, (ask, request) -> reply(ask, hello(), null)));
        commands.put("project", Command.answer(false, (ask, request) -> reply(ask, describeProject(), null)));
        commands.put("device", Command.answer(false, this::requestDevice));
        commands.put("actions", Command.answer(false, (ask, request) -> reply(ask, listActions(), null)));
        commands.put("select", Command.answer(false, this::select));
        commands.put("set", Command.answer(true, this::setParameter));
        commands.put("insert", Command.job(this::insert));
        commands.put("newtrack", Command.job(this::newTrack));
        commands.put("clip", Command.job(this::addClip));
        commands.put("pads", Command.job(this::fillPads));
        commands.put("transpose", Command.job(this::transpose));
        // A job since extension 3.7: a route other than "direct" waits for the command to come back.
        commands.put("action", Command.job(this::runAction));
        // Link version 3.
        commands.put("clicked", Command.answer(false, this::clicked));
        commands.put("recorder", Command.job(this::useRecorder));
        commands.put("selftest", Command.answer(false, (ask, request) -> reply(ask, selfTest("asked"), null)));
        // Extension 3.6: an experiment, which only the live tests send.
        commands.put("arrange", Command.job(this::arrange));
        // Extension 3.8: another experiment, which only the live tests send.
        commands.put("filetrack", Command.job(this::fileTrack));
        // Extension 3.9: recording a launcher clip into the arranger, a way to stop it that doesn't wait behind it, and a
        // probe of whether Bitwig carries out its panels' commands for the link. Only the live tests send them so far.
        commands.put("record", Command.job(this::record));
        commands.put("recordabort", Command.answer(false, this::abortRecording));
        commands.put("probe", Command.job(this::probe));
        // Extension 3.15: the notes of the clips selected in the arranger and the clip launcher, for the Playground's Get
        // chords from Bitwig. It only reads, so it doesn't need changes allowed.
        commands.put("notes", Command.answer(false, this::notes));
        // Extension 3.16: for the Playground playing along with Bitwig. transport says whether Bitwig is playing, where and
        // how fast (it only reads, and the app asks many times a second); stoptracks stops the clips the Playground launched
        // on its tracks. clip takes "replace" and "launch" from 3.16 too (from 3.17 launched in step with the song).
        commands.put("transport", Command.answer(false, this::transportNow));
        commands.put("stoptracks", Command.job(this::stopTracks));
    }

    // ---- Starting and stopping ----

    void init() {
        reports.tidy();

        feature("application", () -> {
            app = host.createApplication();
            app.projectName().markInterested();
            app.hasActiveEngine().markInterested();
        }, () -> "Project \"" + str(app.projectName().get()) + "\", audio engine " + (app.hasActiveEngine().get() ? "on." : "off."));

        feature("project", () -> {
            project = host.getProject();
            project.isModified().markInterested();
        }, () -> project.isModified().get() ? "It has unsaved changes." : "No unsaved changes.");

        feature("transport", () -> {
            transport = host.createTransport();
            transport.tempo().markInterested();
            transport.timeSignature().markInterested();
            transport.isPlaying().markInterested();
            transport.getPosition().markInterested();
        }, () -> Json.number(transport.tempo().getRaw()) + " BPM in " + str(transport.timeSignature().get())
                 + (transport.isPlaying().get() ? ", playing." : ", stopped."));

        // For transport (extension 3.16): where Bitwig is as it plays, and where Play starts from.
        feature("following", () -> {
            transport.playPosition().markInterested();
            transport.playStartPosition().markInterested();
            followed = true;
        }, () -> "Bitwig is at beat " + Json.number(transport.playPosition().get()) + ", and Play starts at beat "
                 + Json.number(transport.playStartPosition().get()) + ".");

        // The track and device selected in Bitwig.
        feature("cursor track", () -> {
            cursorTrack = host.createCursorTrack("MO_LINK", "Music Organiser", 0, 0, true);
            cursorTrack.exists().markInterested();
            cursorTrack.name().markInterested();
            cursorEnd = cursorTrack.endOfDeviceChainInsertionPoint();
        }, () -> cursorTrack.exists().get() ? "Track \"" + str(cursorTrack.name().get()) + "\" is selected." : "No track is selected.");

        feature("cursor device", () -> {
            cursorDevice = cursorTrack.createCursorDevice();
            watchDevice(cursorDevice);
            cursorDevice.position().markInterested();
            cursorDevice.hasDrumPads().markInterested();
            cursorDevice.addDirectParameterIdObserver(ids -> parameterIds(ids));
            cursorDevice.addDirectParameterNameObserver(256, (id, name) -> params.names.put(String.valueOf(id), str(name)));
            cursorDevice.addDirectParameterNormalizedValueObserver((id, value) -> {
                params.values.put(String.valueOf(id), value);
                params.told.merge(String.valueOf(id), 1, Integer::sum);
            });
            displayObserver = cursorDevice.addDirectParameterValueDisplayObserver(64, (id, text) -> {
                params.displays.put(String.valueOf(id), str(text));
            });
        }, () -> cursorDevice.exists().get()
                 ? "Device \"" + str(cursorDevice.name().get()) + "\" is selected, with " + params.ids.size() + " parameters."
                 : "No device is selected.");

        // A Drum Machine's pads, when the selected device is one: pad n plays MIDI note n.
        feature("drum pads", () -> {
            drumPads = cursorDevice.createDrumPadBank(DRUM_PADS);
            drumPads.setIndication(false);
            drumPads.scrollPosition().markInterested();
            for (int p = 0; p < DRUM_PADS; p++) {
                DrumPad pad = (DrumPad) drumPads.getItemAt(p);
                pad.exists().markInterested();
                pad.name().markInterested();
                DeviceBank chain = pad.createDeviceBank(PAD_DEVICES);
                chain.itemCount().markInterested();
                Device first = (Device) chain.getItemAt(0);
                first.exists().markInterested();
                first.name().markInterested();
                first.sampleName().markInterested();
                for (int d = 1; d < PAD_DEVICES; d++) {
                    Device after = (Device) chain.getItemAt(d);
                    after.exists().markInterested();
                    after.deviceType().markInterested();
                }
                padItems[p] = pad;
                padDevices[p] = chain;
                padFirst[p] = first;
                padInserts[p] = pad.insertionPoint();
                padReplace[p] = first.replaceDeviceInsertionPoint();
            }
        }, () -> cursorDevice.hasDrumPads().get()
                 ? "The selected device has drum pads, shown from note " + drumPads.scrollPosition().get() + "."
                 : "The selected device has no drum pads.");

        feature("track bank", () -> {
            tracks = host.createTrackBank(MAX_TRACKS, 0, SCENES, true);
            tracks.itemCount().markInterested();
            for (int t = 0; t < MAX_TRACKS; t++) {
                Track track = (Track) tracks.getItemAt(t);
                trackItems[t] = track;
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
                BooleanValue selected = track.createEqualsValue(cursorTrack);
                selected.markInterested();
                trackSelected[t] = selected;

                DeviceBank devices = track.createDeviceBank(MAX_DEVICES);
                devices.itemCount().markInterested();
                for (int d = 0; d < MAX_DEVICES; d++) watchDevice((Device) devices.getItemAt(d));
                trackDevices[t] = devices;
                trackStarts[t] = track.startOfDeviceChainInsertionPoint();
                trackEnds[t] = track.endOfDeviceChainInsertionPoint();

                ClipLauncherSlotBank slots = track.clipLauncherSlotBank();
                trackSlots[t] = new ClipLauncherSlot[SCENES];
                slotInserts[t] = new InsertionPoint[SCENES];
                for (int s = 0; s < SCENES; s++) {
                    ClipLauncherSlot slot = (ClipLauncherSlot) slots.getItemAt(s);
                    slot.exists().markInterested();
                    slot.hasContent().markInterested();
                    slot.name().markInterested();
                    trackSlots[t][s] = slot;
                    slotInserts[t][s] = slot.replaceInsertionPoint();
                }
            }
        }, () -> tracks.itemCount().get() + " tracks in Bitwig's list; the link sees the first " + MAX_TRACKS + ", with "
                 + MAX_DEVICES + " devices and " + SCENES + " clip slots each.");

        feature("Sampler and Note Transpose pitch", () -> {
            for (int t = 0; t < MAX_TRACKS; t++) {
                Tuning[] tunings = new Tuning[TUNED_DEVICES];
                for (int n = 0; n < TUNED_DEVICES; n++) {
                    Device device = (Device) trackDevices[t].getItemAt(n);
                    tunings[n] = new Tuning(tuning(device, SAMPLER, "TRANSPOSE"), tuning(device, NOTE_TRANSPOSE, "COARSE"));
                }
                trackTunings[t] = tunings;
            }
        }, () -> "Known on the first " + TUNED_DEVICES + " devices of each track.");

        // Newer API, as additions: the app works without them.
        feature("track IDs", () -> {
            for (int t = 0; t < MAX_TRACKS; t++) trackItems[t].channelId().markInterested();
            trackIds = true;
        }, () -> {
            if (!trackItems[0].exists().get()) return "No tracks to show an ID for.";
            String id = str(trackItems[0].channelId().get());
            return id.isEmpty() ? "Bitwig hasn't said the first track's ID." : "The first track's ID is " + id + ".";
        });

        feature("popup browser", () -> {
            popupBrowser = host.createPopupBrowser();
            popupBrowser.exists().markInterested();
        }, () -> popupBrowser.exists().get() ? "Bitwig's browser is open." : "Bitwig's browser is closed.");

        feature("settings", () -> {
            changesSetting = host.getPreferences().getBooleanSetting("Let the app make changes", "Music Organiser", false);
            changesSetting.markInterested();
        }, () -> changesSetting.get() ? "The app may make changes." : "Changes are turned off.");

        feature("remote controls", () -> {
            CursorRemoteControlsPage page = cursorDevice.createCursorRemoteControlsPage(CONTROLS);
            page.getName().markInterested();
            Parameter[] list = new Parameter[CONTROLS];
            for (int i = 0; i < CONTROLS; i++) {
                Parameter control = page.getParameter(i);
                control.exists().markInterested();
                control.name().markInterested();
                control.markInterested();
                control.displayedValue().markInterested();
                list[i] = control;
            }
            controls = list;
            remoteControls = page;
        }, () -> "Page \"" + str(remoteControls.getName().get()) + "\" of the selected device, with "
                 + Arrays.stream(controls).filter(c -> c.exists().get()).count() + " controls.");

        feature("last clicked parameter", () -> {
            LastClickedParameter clicked = host.createLastClickedParameter("MO_CLICKED", "Music Organiser");
            Parameter parameter = clicked.parameter();
            parameter.exists().markInterested();
            parameter.name().markInterested();
            parameter.markInterested();
            parameter.displayedValue().markInterested();
            clicked.isLocked().markInterested();
            clickedParameter = parameter;
            lastClicked = clicked;
        }, () -> clickedParameter.exists().get() ? "\"" + str(clickedParameter.name().get()) + "\" was clicked last."
                                                 : "No parameter has been clicked yet.");

        feature("parameter steps, origins and automation", () -> {
            List<Parameter> all = new ArrayList<>();
            if (controls != null) all.addAll(Arrays.asList(controls));
            if (clickedParameter != null) all.add(clickedParameter);
            for (Parameter parameter : all) {
                parameter.discreteValueCount().markInterested();
                parameter.discreteValueNames().markInterested();
                parameter.getOrigin().markInterested();
                parameter.hasAutomation().markInterested();
            }
            details = !all.isEmpty();
        }, () -> {
            if (!details) throw new LinkError("Neither the remote controls nor the last clicked parameter were made.");
            for (Parameter parameter : controls == null ? new Parameter[0] : controls)
                if (parameter.exists().get())
                    return "\"" + str(parameter.name().get()) + "\": " + steps(parameter.discreteValueCount().get()) + ", origin "
                           + Json.number(parameter.getOrigin().get()) + (parameter.hasAutomation().get() ? ", automated." : ", not automated.");
            return "Read for the remote controls and the last clicked parameter.";
        });

        feature("master recorder", () -> {
            MasterRecorder master = host.createMasterRecorder();
            master.isActive().markInterested();
            master.duration().markInterested();
            recorder = master;
        }, () -> recorder.isActive().get() ? "Recording, for " + recorder.duration().get() + " ms so far." : "Not recording.");

        // For the arrange experiment: the arranger timeline and its clip launcher, the panel layout, the play start, the
        // clip selected in the arranger and its colour, which launcher slot is selected, and each launcher clip's colour.
        feature("arranger", () -> {
            Arranger made = host.createArranger();
            made.isTimelineVisible().markInterested();
            made.isClipLauncherVisible().markInterested();
            app.panelLayout().markInterested();
            transport.playStartPosition().markInterested();
            // The clip's notes aren't read, so its grid can be as small as can be.
            Clip clip = host.createArrangerCursorClip(1, 1);
            clip.exists().markInterested();
            clip.color().markInterested();
            Track on = clip.getTrack();
            on.name().markInterested();
            for (ClipLauncherSlot[] slots : trackSlots) {
                if (slots == null) continue;   // the track bank wasn't made: arrange can't reach a track anyway
                for (ClipLauncherSlot slot : slots) {
                    slot.isSelected().markInterested();
                    slot.color().markInterested();
                }
            }
            arrangerClipTrack = on;
            arrangerClip = clip;
            arranger = made;
        }, () -> "Bitwig shows its " + str(app.panelLayout().get()) + " layout, with the arranger timeline "
                 + (arranger.isTimelineVisible().get() ? "shown" : "hidden") + " and the Arrange layout's clip launcher "
                 + (arranger.isClipLauncherVisible().get() ? "shown" : "hidden") + "; "
                 + (arrangerClip.exists().get() ? "a clip on \"" + str(arrangerClipTrack.name().get()) + "\" is selected in the arranger"
                                                : "no clip is selected in the arranger")
                 + ", and the play start is at beat " + Json.number(transport.playStartPosition().get()) + ".");

        // For notes (extension 3.15): the clips selected in the arranger and in the clip launcher, each with a note grid of
        // its own (the arranger feature's clip has a grid of one step, which shows no notes), the track each is on, and
        // whether that's the selected track.
        feature("clip notes", () -> {
            NoteClip arranged = noteClip("arranger", host.createArrangerCursorClip(NOTE_STEPS, NOTE_KEYS));
            NoteClip launched = noteClip("launcher", host.createLauncherCursorClip(NOTE_STEPS, NOTE_KEYS));
            noteClips = List.of(arranged, launched);
        }, () -> {
            List<String> parts = new ArrayList<>();
            for (NoteClip c : noteClips) {
                String where = "arranger".equals(c.from) ? "the arranger" : "the clip launcher";
                parts.add(c.clip.exists().get() ? "a clip on \"" + str(c.track.name().get()) + "\" is selected in " + where + ", with "
                                                  + c.notes.size() + " notes starting in its grid"
                                                : "no clip is selected in " + where);
            }
            String said = String.join("; ", parts) + ".";
            return Character.toUpperCase(said.charAt(0)) + said.substring(1);
        });

        // For the filetrack experiment (extension 3.8): where a file goes when it's dropped after each track.
        feature("files as tracks", () -> {
            InsertionPoint[] made = new InsertionPoint[MAX_TRACKS];
            for (int t = 0; t < MAX_TRACKS; t++) {
                if (trackItems[t] == null) return;   // the track bank wasn't made: filetrack can't reach a track anyway
                made[t] = trackItems[t].afterTrackInsertionPoint();
            }
            trackAfter = made;
        }, () -> trackAfter == null ? "The track bank wasn't made, so there's nowhere to insert a file."
                                    : "A file can go after any of the first " + MAX_TRACKS + " tracks, as a new track.");

        // For the record job (extension 3.9): the transport's switches it sets and puts back, where Bitwig is playing, the
        // master track (muted while the job plays), and each track's playing notes and launcher slots as they play.
        feature("recording into the arranger", () -> {
            if (trackItems[MAX_TRACKS - 1] == null) throw new LinkError("The track bank wasn't made, so there's no track to record.");
            for (SettableBooleanValue value : switches()) value.markInterested();
            transport.automationWriteMode().markInterested();
            transport.preRoll().markInterested();
            transport.defaultLaunchQuantization().markInterested();
            transport.playPosition().markInterested();
            transport.playStartPosition().markInterested();
            project.hasArmedTracks().markInterested();
            MasterTrack made = host.createMasterTrack(0);
            made.mute().markInterested();
            for (int t = 0; t < MAX_TRACKS; t++) {
                trackItems[t].isStopped().markInterested();
                trackItems[t].playingNotes().markInterested();
                for (ClipLauncherSlot slot : trackSlots[t]) {
                    slot.isPlaying().markInterested();
                    slot.isPlaybackQueued().markInterested();
                    slot.isRecording().markInterested();
                }
            }
            master = made;
        }, () -> "Arranger record is " + (transport.isArrangerRecordEnabled().get() ? "on" : "off") + ", pre-roll "
                 + str(transport.preRoll().get()) + ", launch quantize " + str(transport.defaultLaunchQuantization().get())
                 + ", automation write mode " + str(transport.automationWriteMode().get()) + "; the master track is "
                 + (master.mute().get() ? "muted." : "not muted."));

        // The routes that run Bitwig's commands other than straight away (see ROUTES): the loopback's MIDI ports, and two
        // buttons for each command of ROUTED Bitwig has, which notes on channel 16 coming back through the loopback press.
        feature("command routes", () -> {
            MidiIn in = host.getMidiInPort(0);
            MidiOut out = host.getMidiOutPort(0);
            in.setSysexCallback(this::received);
            HardwareSurface made = host.createHardwareSurface();
            made.setPhysicalSize(60, 20);
            Map<String, Integer> found = new LinkedHashMap<>();
            for (int i = 0; i < ROUTED.size(); i++) {
                String id = ROUTED.get(i);
                Action action = app.getAction(id);
                if (action == null) continue;
                // Bitwig runs the command bound to this one itself, as for DrivenByMoss's buttons; the link only hears of the press.
                HardwareButton bound = made.createHardwareButton("MO_BUTTON_" + id);
                bound.pressedAction().setActionMatcher(in.createNoteOnActionMatcher(CHANNEL, NOTE_BOUND + i));
                bound.pressedAction().addBinding(action);
                bound.pressedAction().addBinding(host.createAction(() -> pressed(id, "button"), () -> "Music Organiser: " + id + " pressed"));
                // The link runs the command for a press of this one, while Bitwig handles it.
                HardwareButton code = made.createHardwareButton("MO_CODE_" + id);
                code.pressedAction().setActionMatcher(in.createNoteOnActionMatcher(CHANNEL, NOTE_CODE + i));
                code.pressedAction().setBinding(host.createAction(() -> pressed(id, "buttonCode"), () -> "Music Organiser: run " + id));
                found.put(id, i);
            }
            midiIn = in;
            midiOut = out;
            surface = made;
            buttons = found;
        }, () -> "Buttons for " + (buttons.isEmpty() ? "none of Bitwig's commands" : String.join(", ", buttons.keySet())) + " on MIDI channel "
                 + (CHANNEL + 1) + " (notes from " + NOTE_BOUND + " and from " + NOTE_CODE + "). "
                 + (loopback ? "The link has had its own MIDI back, so the loopback works."
                             : "The link hasn't had its own MIDI back, so its MIDI in and out may not be set (or their port isn't working), "
                               + "and the routes through MIDI need them. Bitwig 6.1.3 has only saved for the link once they were set. " + LOOPBACK_SETUP));

        feature("remote connection", () -> {
            socket = host.createRemoteConnection("Music Organiser", PORT);
            socket.setClientConnectCallback(this::connected);
            port = socket.getPort();
            listening = true;
        }, () -> {
            int now = knownPort();
            if (now > 0 && now != PORT)
                throw new LinkError("Port " + PORT + " is taken, so Bitwig gave the link port " + now + ", where the app doesn't look. " + TWICE);
            if (now <= 0) return "Waiting for the app on port " + PORT + ", though Bitwig hasn't said yet which port it gave the link.";
            return "Waiting for the app on port " + now + ", with " + clients.size() + " connected now.";
        });
        if (listening) {
            host.println("Music Organiser: waiting for the app on port " + (port > 0 ? port : PORT) + ".");
            warnIfPortTaken();
        } else {
            host.showPopupNotification("Music Organiser: Bitwig couldn't listen for the app. The self-check in " + reports.folder()
                                       + " says why.");
        }

        Map<String, Object> start = event("start");
        start.put("extension", MusicOrganiserExtensionDefinition.VERSION);
        if (!loadedFrom.isEmpty()) start.put("loadedFrom", loadedFrom);
        start.put("bitwig", str(host.getHostVersion()));
        start.put("api", host.getHostApiVersion());
        start.put("port", port);
        reports.log(start);
        // So the link knows early on whether the loopback works (hello's "loopback").
        ping();
        host.scheduleTask(() -> {
            try {
                selfTest("start");
            } catch (RuntimeException | LinkageError e) {
                host.errorln("Music Organiser: the self-check failed (" + e + ").");
            }
        }, SELF_CHECK_DELAY);
    }

    /** Bitwig's flush: runs the commands waiting for it (the "flush" route). */
    void flush() {
        if (flushing.isEmpty()) return;
        List<Sent> due = new ArrayList<>(flushing);
        flushing.clear();
        for (Sent sent : due) run(sent);
    }

    void exit() {
        // A recording mustn't go on without the link: record off, Bitwig stopped, and its settings put back, as far as can be.
        if (recording != null) {
            Recording stopping = recording;
            stopping.emergency();
            stopping.done = true;
        }
        for (Client client : clients) client.open = false;
        clients.clear();
        jobs.clear();
        dropSent();
        reports.log(event("stop"));
    }

    private void watchDevice(Device device) {
        device.exists().markInterested();
        device.name().markInterested();
        device.presetName().markInterested();
        device.isPlugin().markInterested();
        device.deviceType().markInterested();
        device.isEnabled().markInterested();
    }

    /** One of a Bitwig device's parameters, for whenever `device` is that device (by the ID Bitwig saves it under). */
    private Parameter tuning(Device device, String id, String param) {
        Parameter parameter = device.createSpecificBitwigDevice(UUID.fromString(id)).createParameter(param);
        parameter.exists().markInterested();
        parameter.markInterested();
        parameter.displayedValue().markInterested();
        return parameter;
    }

    /** The selected device's parameter IDs changed: forget what's known of parameters it no longer has. */
    private void parameterIds(Object ids) {
        // A String[]; Bitwig's own build of the API has no generic types, so the callback is handed an Object.
        Object[] array = ids instanceof Object[] ? (Object[]) ids : new Object[0];
        List<String> list = new ArrayList<>(array.length);
        for (Object id : array) list.add(String.valueOf(id));
        params.ids = list;
        Set<String> kept = new HashSet<>(list);
        params.names.keySet().retainAll(kept);
        params.values.keySet().retainAll(kept);
        params.displays.keySet().retainAll(kept);
        params.told.keySet().retainAll(kept);
        params.answered.keySet().retainAll(kept);
    }

    // ---- Talking to the app ----

    private void connected(RemoteConnection connection) {
        Client client = new Client(connection);
        clients.add(client);
        knownPort();
        connection.setDisconnectCallback(() -> {
            client.open = false;
            clients.remove(client);
            reports.log(event("disconnected"));
        });
        connection.setReceiveCallback(data -> handle(client, Framing.text(data)));
        reports.log(event("connected"));
        Map<String, Object> hello = new LinkedHashMap<>();
        hello.put("event", "hello");
        hello.put("result", greeting());
        send(client, hello);
        ping();
    }

    /**
     * The port Bitwig gave the link, asked again while Bitwig hasn't said: Bitwig 6.1.3 says -1 straight after making
     * the connection. The app connecting shows it's 19791, the only port the app tries. Warns in Bitwig when it's
     * another.
     */
    private int knownPort() {
        if (port <= 0 && socket != null) {
            port = socket.getPort();
            if (port <= 0 && !clients.isEmpty()) port = PORT;
        }
        warnIfPortTaken();
        return port;
    }

    private void warnIfPortTaken() {
        if (port <= 0 || port == PORT || portWarned) return;
        portWarned = true;
        host.showPopupNotification("Music Organiser: port " + PORT + " is taken, so the app can't connect. " + TWICE);
    }

    /** Handles one request from the app. */
    void handle(Client client, String text) {
        long started = clock.millis();
        Object request;
        try {
            request = Json.parse(text);
        } catch (Json.JsonException e) {
            reply(new Ask(client, null, null, started), null, "The request isn't JSON: " + text.substring(0, Math.min(80, text.length())));
            return;
        }
        Map<String, Object> map = asMap(request);
        Object id = map != null && map.get("id") != null ? map.get("id") : null;
        // What JavaScript's request.cmd was, for the message: undefined unless the request is an object.
        Object cmd = map != null ? field(map, "cmd") : Json.truthy(request) ? Json.UNDEFINED : request;
        Ask ask = new Ask(client, id, logName(cmd), started);
        try {
            Command command = cmd instanceof String ? commands.get(cmd) : null;
            if (map == null || command == null) throw new LinkError("Unknown command: " + Json.js(cmd));
            checkAllowed(map, command.changes());
            if (command.job() != null) {
                jobs.add(new Job(ask, map, command.job(), command.changes()));
                if (currentJob == null) nextJob();
            } else {
                command.run().run(ask, map);
            }
        } catch (RuntimeException | LinkageError e) {
            reply(ask, null, failure(e));
        }
    }

    /**
     * Refuses a request that may not go ahead now. A request that carries `onlyIn` (a project's name) only goes ahead
     * while Bitwig has that project open, and a change only while "Let the app make changes" is on. This is checked
     * when a request arrives, when a change starts (it may have waited behind others) and before each step of a change
     * after a wait, since the track bank, the cursor track and the cursor device follow whichever project is in front,
     * and someone may bring their own to the front, or turn changes off, meanwhile.
     */
    private void checkAllowed(Map<String, Object> request, boolean changes) {
        Object only = field(request, "onlyIn");
        if (!absent(only)) {
            if (!(only instanceof String)) throw new LinkError("onlyIn has to be a project's name.");
            String open = str(app.projectName().get());
            if (!open.equals(only))
                throw new LinkError("Bitwig has \"" + open + "\" open, not \"" + only + "\", so the link stopped before changing anything there.");
        }
        if (changes && !changesSetting.get()) throw new LinkError(CHANGES_OFF);
    }

    /**
     * Runs the next change waiting, if any. A change whose app has closed its connection meanwhile (it gave up waiting)
     * is left out (extension 3.18): the app never hears it was made, so it would ask again and get it twice. A stoptracks
     * still runs: stopping twice does no harm, and the app sends it as it closes, so it often waits behind a clip.
     */
    private void nextJob() {
        Job next = jobs.poll();
        while (next != null && !next.ask().client().open && !"stoptracks".equals(next.request().get("cmd"))) {
            reply(next.ask(), null, APP_GONE);
            next = jobs.poll();
        }
        final Job job = next;
        currentJob = job;
        if (job == null) return;
        Finish finish = new Finish() {
            private boolean finished;

            @Override
            public void done(Object result, String error) {
                if (finished) return;
                finished = true;
                // Nothing the change sent on its way by a route runs once it has ended, whatever comes back later.
                dropSent();
                reply(job.ask(), result, error);
                nextJob();
            }
        };
        attempt(() -> {
            checkAllowed(job.request(), job.changes());
            job.run().run(job.request(), finish);
        }, finish);
    }

    /**
     * Before a step of the change running now that comes after a wait: stops the change (with an error for `attempt`
     * to hand to its finish) unless it may still go ahead. Every step after a wait goes through waitFor or settle,
     * which call this, so nothing a change does after a wait can reach another project.
     */
    private void carryOn(Job job) {
        if (job != null) checkAllowed(job.request(), job.changes());
    }

    /** The change running now, or null (the tests' stand-in Bitwig watches for it). */
    Job runningJob() {
        return currentJob;
    }

    /** Runs one step of a change, ending the change with an error if the step fails. */
    private void attempt(Runnable step, Finish finish) {
        try {
            step.run();
        } catch (RuntimeException | LinkageError e) {
            finish.done(null, failure(e));
        }
    }

    private String failure(Throwable e) {
        if (e instanceof LinkError) return e.getMessage();
        StringWriter trace = new StringWriter();
        e.printStackTrace(new PrintWriter(trace));
        host.errorln("Music Organiser: " + trace);
        return "The Bitwig extension failed: " + e;
    }

    /**
     * Bitwig makes changes after the extension returns, and tells it a moment later, so a change waits: `test`
     * is checked every 50 ms until it's true or `ms` have passed, and `then` gets whether it came true. Each time,
     * the change stops instead if it may no longer go ahead (see carryOn).
     */
    private void waitFor(BooleanSupplier test, long ms, Then then, Finish finish) {
        long start = clock.millis();
        Job job = currentJob;
        Runnable check = new Runnable() {
            @Override
            public void run() {
                if (job != currentJob) return;   // a step left over from a change that has ended does nothing
                attempt(() -> {
                    carryOn(job);
                    boolean ok = test.getAsBoolean();
                    if (ok || clock.millis() - start >= ms) then.run(ok);
                    else host.scheduleTask(this, 50);
                }, finish);
            }
        };
        host.scheduleTask(check, 50);
    }

    /**
     * Waits until what `look` sees stops changing (the same three times running), since Bitwig reports a change
     * bit by bit, then hands it to `then`. Each time, the change stops instead if it may no longer go ahead (see
     * carryOn).
     */
    private <T> void settle(Supplier<T> look, Consumer<T> then, Finish finish) {
        long start = clock.millis();
        Job job = currentJob;
        Runnable check = new Runnable() {
            private String last;
            private int same;

            @Override
            public void run() {
                if (job != currentJob) return;   // a step left over from a change that has ended does nothing
                attempt(() -> {
                    carryOn(job);
                    T seen = look.get();
                    String text = Json.write(seen);
                    same = text.equals(last) ? same + 1 : 0;
                    last = text;
                    if (same >= 2 || clock.millis() - start >= CHANGE_WAIT) then.accept(seen);
                    else host.scheduleTask(this, 50);
                }, finish);
            }
        };
        host.scheduleTask(check, 50);
    }

    /**
     * Answers a request: with its result, or with an error message for the app. Each answer goes in the log, apart from
     * a hello that went well: the app says hello every couple of seconds to see Bitwig is there, which would fill the
     * log with tens of thousands of lines a day.
     */
    private void reply(Ask ask, Object result, String error) {
        Map<String, Object> message = new LinkedHashMap<>();
        message.put("id", ask.id());
        message.put("ok", error == null);
        if (error != null) message.put("error", error);
        else message.put("result", result);
        send(ask.client(), message);
        // The app asks hello every few seconds, and transport many times a second while the Playground follows Bitwig.
        if (error == null && ("hello".equals(ask.cmd()) || "transport".equals(ask.cmd()))) return;

        Map<String, Object> line = new LinkedHashMap<>();
        line.put("cmd", ask.cmd());
        line.put("ms", clock.millis() - ask.started());
        line.put("ok", error == null);
        if (error != null) line.put("error", error);
        reports.log(line);
    }

    private void send(Client client, Map<String, Object> message) {
        if (!client.open) return;
        try {
            client.connection.send(Framing.message(Json.write(message)));
        } catch (Exception e) {   // Exception: some builds of the API declare an IOException here
            client.open = false;
            clients.remove(client);
            host.errorln("Music Organiser: lost the connection to the app (" + e + ").");
        }
    }

    private static Map<String, Object> event(String name) {
        Map<String, Object> line = new LinkedHashMap<>();
        line.put("event", name);
        return line;
    }

    /** The command as the log shows it: short, and only what the app sent. */
    private static String logName(Object cmd) {
        if (cmd == Json.UNDEFINED) return null;
        String name = cmd instanceof String ? (String) cmd : Json.js(cmd);
        return name.length() > 40 ? name.substring(0, 40) : name;
    }

    // ---- What the app can ask for ----

    private Map<String, Object> hello() {
        Map<String, Object> m = new LinkedHashMap<>();
        m.put("link", LINK_VERSION);
        m.put("product", str(host.getHostProduct()));
        m.put("version", str(host.getHostVersion()));
        m.put("api", host.getHostApiVersion());
        m.put("platform", str(host.getPlatformType()));
        m.put("port", port);
        m.put("project", str(app.projectName().get()));
        m.put("modified", project.isModified().get());
        m.put("engine", app.hasActiveEngine().get());
        m.put("changes", changesSetting.get());
        // This link honours a request's onlyIn (see checkAllowed).
        m.put("onlyIn", true);
        // Additions in link version 3. ("engine" already says whether Bitwig's audio engine is on.)
        m.put("runtime", "java");
        m.put("extension", MusicOrganiserExtensionDefinition.VERSION);
        if (!loadedFrom.isEmpty()) m.put("loadedFrom", loadedFrom);
        // Extension 3.7: whether the link's own MIDI has come back through the loopback (see ROUTES).
        m.put("loopback", loopback);
        return m;
    }

    /** The file a class was loaded from, when it's a file (an archive such as the .bwextension), else "". */
    static String loadedFrom(Class<?> type) {
        try {
            CodeSource source = type.getProtectionDomain().getCodeSource();
            if (source == null || source.getLocation() == null) return "";
            Path file = Path.of(uri(source.getLocation()));
            return Files.isRegularFile(file) ? file.toString() : "";
        } catch (URISyntaxException | RuntimeException e) {
            // Not a file: URL, or Bitwig doesn't allow asking.
            return "";
        }
    }

    /**
     * A URL as a URI. Bitwig makes its extensions' URLs from their files without escaping them (File.toURL), so one in a
     * folder with a space in its name ("Bitwig Studio") isn't a URI as it stands: its parts are escaped here instead.
     */
    static URI uri(URL url) throws URISyntaxException {
        try {
            return url.toURI();
        } catch (URISyntaxException e) {
            String authority = url.getAuthority();
            return new URI(url.getProtocol(), authority == null || authority.isEmpty() ? null : authority, url.getPath(), null, null);
        }
    }

    /** The greeting for a new connection: what's known, even when part of Bitwig's API didn't start. */
    private Map<String, Object> greeting() {
        try {
            return hello();
        } catch (RuntimeException | LinkageError e) {
            Map<String, Object> m = new LinkedHashMap<>();
            m.put("link", LINK_VERSION);
            m.put("product", str(host.getHostProduct()));
            m.put("version", str(host.getHostVersion()));
            m.put("api", host.getHostApiVersion());
            m.put("platform", str(host.getPlatformType()));
            m.put("port", port);
            m.put("onlyIn", true);
            m.put("runtime", "java");
            m.put("extension", MusicOrganiserExtensionDefinition.VERSION);
            if (!loadedFrom.isEmpty()) m.put("loadedFrom", loadedFrom);
            m.put("loopback", loopback);
            m.put("error", failure(e) + " Ask for a selftest to see what's wrong.");
            return m;
        }
    }

    private Map<String, Object> describeProject() {
        List<Object> list = new ArrayList<>();
        for (int t = 0; t < MAX_TRACKS; t++) {
            Track track = trackItems[t];
            if (track.exists().get()) list.add(describeTrack(t, track, trackDevices[t]));
        }
        Map<String, Object> m = new LinkedHashMap<>();
        m.put("name", str(app.projectName().get()));
        m.put("modified", project.isModified().get());
        m.put("tempo", transport.tempo().getRaw());
        m.put("timeSignature", str(transport.timeSignature().get()));
        m.put("playing", transport.isPlaying().get());
        m.put("position", transport.getPosition().get());
        m.put("trackCount", tracks.itemCount().get());
        m.put("tracks", list);
        return m;
    }

    private Map<String, Object> describeTrack(int index, Track track, DeviceBank devices) {
        List<Object> list = new ArrayList<>();
        for (int d = 0; d < MAX_DEVICES; d++) {
            Device device = (Device) devices.getItemAt(d);
            if (device.exists().get()) list.add(describeDevice(d, device));
        }
        Map<String, Object> m = new LinkedHashMap<>();
        m.put("index", index);
        // The ID Bitwig keeps the track under (link version 3), when it has said.
        String id = trackIds ? str(track.channelId().get()) : "";
        if (!id.isEmpty()) m.put("id", id);
        m.put("name", str(track.name().get()));
        m.put("type", str(track.trackType().get()));
        m.put("group", track.isGroup().get());
        m.put("color", colour(track.color()));
        m.put("active", track.isActivated().get());
        m.put("mute", track.mute().get());
        m.put("solo", track.solo().get());
        m.put("arm", track.arm().get());
        m.put("volume", str(track.volume().displayedValue().get()));
        m.put("volumeValue", track.volume().get());
        m.put("pan", str(track.pan().displayedValue().get()));
        m.put("panValue", track.pan().get());
        m.put("deviceCount", devices.itemCount().get());
        m.put("devices", list);
        return m;
    }

    private static Map<String, Object> describeDevice(int index, Device device) {
        Map<String, Object> m = new LinkedHashMap<>();
        m.put("index", index);
        m.put("name", str(device.name().get()));
        m.put("preset", str(device.presetName().get()));
        m.put("kind", str(device.deviceType().get()));
        m.put("plugin", device.isPlugin().get());
        m.put("enabled", device.isEnabled().get());
        return m;
    }

    /**
     * The selected device with all its parameters. Bitwig sends the display texts ("-6.0 dB") only once they're
     * asked for, so the reply waits for them (up to DEVICE_WAIT).
     */
    private void requestDevice(Ask ask, Map<String, Object> request) {
        if (!cursorDevice.exists().get()) {
            Map<String, Object> m = new LinkedHashMap<>();
            m.put("track", str(cursorTrack.name().get()));
            m.put("device", null);
            m.put("params", List.of());
            m.put("drumPads", false);
            reply(ask, m, null);
            return;
        }
        deviceWaiters.add(ask);
        if (deviceWaiters.size() > 1) return;   // already waiting: this request gets the same answer
        try {
            params.displays.clear();
            displayObserver.setObservedParameterIds(params.ids.toArray(new String[0]));
            deviceWaitStart = clock.millis();
            host.scheduleTask(this::checkDevice, 50);
        } catch (RuntimeException | LinkageError e) {
            deviceWaiters.clear();   // so the next request doesn't wait behind this one for ever
            throw e;
        }
    }

    private void checkDevice() {
        int missing = 0;
        for (String id : params.ids)
            if (!params.displays.containsKey(id)) missing++;
        if (missing > 0 && clock.millis() - deviceWaitStart < DEVICE_WAIT) {
            host.scheduleTask(this::checkDevice, 50);
            return;
        }
        Map<String, Object> result = null;
        String error = null;
        try {
            result = describeSelectedDevice();
            displayObserver.setObservedParameterIds(null);
        } catch (RuntimeException | LinkageError e) {
            // Every waiting request still gets an answer, and the next one starts afresh.
            error = failure(e);
        }
        List<Ask> waiters = new ArrayList<>(deviceWaiters);
        deviceWaiters.clear();
        for (Ask waiter : waiters) reply(waiter, result, error);
    }

    private Map<String, Object> describeSelectedDevice() {
        // Which remote control (if any) is each parameter: Bitwig's API tells steps, origins and automation only
        // for those, and doesn't say which parameter a control is, so a control is matched to the one parameter
        // with its name and value. A plugin's control is the one parameter with its name, whatever the value: Bitwig
        // may not have told the link the parameter's new value, which its control has. (A Bitwig device's control
        // may be a device's inside it, of the same name.)
        Map<String, Parameter> controlOf = new HashMap<>();
        Map<Parameter, String> paramOf = new HashMap<>();
        boolean plugin = cursorDevice.exists().get() && cursorDevice.isPlugin().get();
        if (controls != null) {
            for (Parameter control : controls) {
                if (!control.exists().get()) continue;
                String match = null, named = null;
                int found = 0, names = 0;
                for (String id : params.ids) {
                    if (!str(control.name().get()).equals(params.names.get(id))) continue;
                    named = id;
                    names++;
                    Double value = params.values.get(id);
                    double v = control.get();
                    if (value == null || !(Math.abs(value - v) < 1e-4 || (value.isNaN() && Double.isNaN(v)))) continue;
                    match = id;
                    found++;
                }
                if (names == 1 && plugin) {
                    match = named;
                    found = 1;
                }
                if (found == 1 && !controlOf.containsKey(match)) {
                    controlOf.put(match, control);
                    paramOf.put(control, match);
                }
            }
        }

        List<Object> list = new ArrayList<>();
        for (String id : params.ids) {
            Map<String, Object> p = new LinkedHashMap<>();
            p.put("id", id);
            p.put("name", params.names.getOrDefault(id, ""));
            Parameter control = controlOf.get(id);
            Double value = params.values.get(id);
            String display = params.displays.get(id);
            boolean stale = staleValue(id, value, display, control);
            // NaN (not readable) becomes null in JSON. A control's value is Bitwig's own; the parameter's may be out of date.
            p.put("value", control != null ? (Object) control.get() : stale ? null : value);
            p.put("display", display);
            if (stale && control == null) p.put("stale", true);
            if (control != null) addDetails(p, control);
            list.add(p);
        }
        boolean exists = cursorDevice.exists().get();
        Map<String, Object> m = new LinkedHashMap<>();
        m.put("track", str(cursorTrack.name().get()));
        m.put("device", exists ? describeDevice(-1, cursorDevice) : null);
        m.put("params", list);
        m.put("drumPads", exists && cursorDevice.hasDrumPads().get());
        if (remoteControls != null) {
            // Link version 3: the device's remote controls on the page Bitwig shows, with what's known of each.
            List<Object> shown = new ArrayList<>();
            for (int i = 0; i < CONTROLS; i++) {
                Parameter control = controls[i];
                if (!control.exists().get()) continue;
                Map<String, Object> c = new LinkedHashMap<>();
                c.put("index", i);
                c.put("name", str(control.name().get()));
                c.put("value", control.get());
                c.put("display", str(control.displayedValue().get()));
                addDetails(c, control);
                c.put("param", paramOf.get(control));
                shown.add(c);
            }
            m.put("page", str(remoteControls.getName().get()));
            m.put("controls", shown);
        }
        return m;
    }

    /**
     * Whether the value Bitwig last told the link for a parameter is out of date: its text has changed since an answer
     * gave it, and Bitwig hasn't told the value since (the 20:37 live run on 2026-10-18: Bitwig 6.1.5 showed the new text
     * of a plugin's knob, turned by the plugin or by the link, but never told its new value). It stays so until Bitwig
     * tells it again. A remote control has Bitwig's own value, which says whether the parameter's is behind.
     */
    private boolean staleValue(String id, Double value, String display, Parameter control) {
        int told = params.told.getOrDefault(id, 0);
        Answered before = params.answered.get(id);
        boolean stale;
        if (control != null) stale = value != null && !value.isNaN() && Math.abs(value - control.get()) >= 1e-4;
        else stale = before != null && before.told() == told
                     && (before.stale() || display != null && before.display() != null && !display.equals(before.display()));
        params.answered.put(id, new Answered(told, display != null || before == null ? display : before.display(), stale));
        return stale;
    }

    /** A parameter's steps (-1 when it's continuous), their names, its origin and whether it's automated. */
    private void addDetails(Map<String, Object> m, Parameter parameter) {
        if (!details) return;
        try {
            int steps = parameter.discreteValueCount().get();
            String[] names = parameter.discreteValueNames().get();
            double origin = parameter.getOrigin().get();
            boolean automated = parameter.hasAutomation().get();
            m.put("steps", steps);
            if (steps > 0 && names != null && names.length > 0) m.put("stepNames", Arrays.asList(names));
            m.put("origin", origin);
            m.put("automated", automated);
        } catch (RuntimeException e) {
            // Best effort: the rest of the answer matters more.
        }
    }

    private List<Object> listActions() {
        Action[] all = app.getActions();
        List<Object> list = new ArrayList<>();
        for (Action action : all) {
            ActionCategory category = action.getCategory();
            Map<String, Object> m = new LinkedHashMap<>();
            m.put("id", str(action.getId()));
            m.put("name", str(action.getName()));
            m.put("category", category != null ? str(category.getName()) : "");
            m.put("menu", str(action.getMenuItemText()));
            list.add(m);
        }
        return list;
    }

    /** Selects a track from the project listing (by its index there) and, optionally, one of its devices. */
    private void select(Ask ask, Map<String, Object> request) {
        int t = checkTrack(field(request, "track"));
        Object d = field(request, "device");
        Device device = absent(d) ? null : trackDevice(t, d);
        cursorTrack.selectChannel(trackItems[t]);
        if (device != null) cursorDevice.selectDevice(device);
        reply(ask, Map.of(), null);
    }

    /** Sets one of the selected device's parameters, by its ID, to a value from 0 to 1. */
    private void setParameter(Ask ask, Map<String, Object> request) {
        if (!cursorDevice.exists().get()) throw new LinkError("No device is selected in Bitwig.");
        Object param = field(request, "param");
        if (!(param instanceof String) || !params.ids.contains(param))
            throw new LinkError("The selected device has no parameter " + Json.js(param) + ".");
        Object value = field(request, "value");
        if (!(value instanceof Double) || !((Double) value >= 0 && (Double) value <= 1))
            throw new LinkError("The value has to be a number from 0 to 1.");
        // Bitwig takes the value as a step out of (resolution - 1).
        cursorDevice.setDirectParameterValueNormalized((String) param, Math.round((Double) value * 1000000), 1000001);
        reply(ask, Map.of(), null);
    }

    /**
     * Runs one of Bitwig's actions by its ID (as listed by the actions command), e.g. saving the project, by the route
     * the request asks for as `via` (DEFAULT_ROUTE, Bitwig's flush, when it doesn't say: see ROUTES). The answer (extension 3.7) says
     * which project Bitwig had in front just before the action ran, as `project`: on Bitwig 6.1.3 Save once left the
     * test project unsaved while another project was open too, and which one is in front may be why. It's left out when
     * the link can't read it (the application part of the API didn't set up). `via` is the route, and `ran` whether the
     * action got run by it: a route other than "direct" may not come back (the loopback isn't set up, say), and then
     * the link didn't run it. Whether Bitwig then did what the action does is for the app to see.
     */
    private void runAction(Map<String, Object> request, Finish finish) {
        String route = route(request);
        Object id = field(request, "action");
        Action action = id instanceof String ? app.getAction((String) id) : null;
        if (action == null) throw new LinkError("Bitwig has no action " + Json.js(id) + ".");
        String front = frontProject();
        runBy(route, (String) id, action::invoke, ran -> {
            Map<String, Object> m = new LinkedHashMap<>();
            if (front != null) m.put("project", front);
            m.put("via", route);
            m.put("ran", ran);
            finish.ok(m);
        }, finish);
    }

    // ---- Routes (extension 3.7) ----

    /**
     * The route a request asks for as `via`: DEFAULT_ROUTE when it doesn't say. Refuses one that isn't in ROUTES, and one
     * that needs the loopback's ports and buttons when Bitwig wouldn't let the link set them up.
     */
    private String route(Map<String, Object> request) {
        Object via = field(request, "via");
        if (absent(via)) return DEFAULT_ROUTE;
        if (!(via instanceof String) || !ROUTES.contains(via))
            throw new LinkError("The route (via) has to be one of " + String.join(", ", ROUTES) + ", not " + Json.js(via) + ".");
        String route = (String) via;
        if (needsLoopback(route) && buttons == null)
            throw new LinkError("This Bitwig didn't let the link set up its MIDI ports and buttons, so it can't run a command via "
                                + route + ". A selftest says why.");
        return route;
    }

    /** Whether a route goes through the loopback (MIDI out and back in): all but "direct" and "flush". */
    private static boolean needsLoopback(String route) {
        return !route.equals(DIRECT) && !route.equals("flush");
    }

    /** Refuses a button route for a command the link has no buttons for (Bitwig had no such action when the link started). */
    private void checkButtons(String route, List<String> ids) {
        if (!route.equals("button") && !route.equals("buttonCode")) return;
        for (String id : ids)
            if (!buttons.containsKey(id))
                throw new LinkError("The link has buttons for " + (buttons.isEmpty() ? "none of Bitwig's commands" : String.join(", ", buttons.keySet()))
                                    + ", not " + Json.js(id) + ", so it can't run that via " + route + ".");
    }

    /**
     * Runs one of Bitwig's commands (`run`, which is Bitwig's action `id` or does the same) by a route (see ROUTES), then
     * hands `then` whether it ran. "direct" runs it now. Any other route sends it on its way and waits up to
     * LOOPBACK_WAIT for it to come back: when it doesn't, the link drops it, so it never runs later, and `then` gets
     * false. Only the "button" route can't be held back like that, as Bitwig runs the command bound to the button
     * itself; the link just hears of the press.
     */
    private void runBy(String route, String id, Runnable run, Then then, Finish finish) {
        if (route.equals(DIRECT)) {
            run.run();
            then.run(true);
            return;
        }
        checkButtons(route, List.of(id));
        Sent sent = new Sent(route, id, run);
        switch (route) {
            case "flush" -> {
                flushing.add(sent);
                host.requestFlush();
            }
            case "midi" -> {
                int token = nextToken();
                tokens.put(token, sent);
                midiOut.sendSysex(message(token));
            }
            default -> {
                int note = (route.equals("button") ? NOTE_BOUND : NOTE_CODE) + buttons.get(id);
                presses.put(route + " " + id, sent);
                // A press and a release, as a button would send.
                midiOut.sendMidi(0x90 | CHANNEL, note, 127);
                midiOut.sendMidi(0x80 | CHANNEL, note, 0);
            }
        }
        waitFor(() -> sent.ran, LOOPBACK_WAIT, ran -> {
            if (!ran) drop(sent);
            then.run(ran);
        }, finish);
    }

    /**
     * Runs a command that came back by its route, unless the link gave up on it already, or the change may no longer go
     * ahead (another project came to the front meanwhile, say: the change then stops at its next step, saying why).
     */
    private void run(Sent sent) {
        if (sent.dropped) return;
        try {
            carryOn(currentJob);
        } catch (RuntimeException e) {
            drop(sent);
            return;
        }
        try {
            sent.run.run();
            sent.ran = true;
        } catch (RuntimeException | LinkageError e) {
            host.errorln("Music Organiser: couldn't run " + sent.id + " via " + sent.route + " (" + e + ").");
        }
    }

    /** Gives up on a command sent by a route: if it comes back after all, nothing runs. */
    private void drop(Sent sent) {
        sent.dropped = true;
        tokens.values().remove(sent);
        presses.values().remove(sent);
        flushing.remove(sent);
    }

    /** Gives up on every command on its way by a route (when a change ends, and when Bitwig stops the link). */
    private void dropSent() {
        for (Sent sent : tokens.values()) sent.dropped = true;
        for (Sent sent : presses.values()) sent.dropped = true;
        for (Sent sent : flushing) sent.dropped = true;
        tokens.clear();
        presses.clear();
        flushing.clear();
    }

    /** The next SysEx message's number: 1 up to the most three 7-bit bytes hold, then 1 again. */
    private int nextToken() {
        lastToken = lastToken % 0x1FFFFF + 1;
        return lastToken;
    }

    /** The SysEx message the link sends itself with a number (see SYSEX_START), written as hex the way Bitwig takes it. */
    static String message(int token) {
        return String.format("F0 7D 4D 4F 42 %02X %02X %02X F7", (token >> 14) & 0x7F, (token >> 7) & 0x7F, token & 0x7F);
    }

    /** Sends the link a SysEx message that nothing waits for, so it learns whether the loopback works (hello's `loopback`). */
    private void ping() {
        if (midiOut == null) return;
        try {
            midiOut.sendSysex(message(nextToken()));
        } catch (RuntimeException | LinkageError e) {
            host.errorln("Music Organiser: couldn't send a message through the loopback (" + e + ").");
        }
    }

    /**
     * A SysEx message on the loopback's MIDI in, as hex. One of the link's own (see message) says the loopback works,
     * and runs the command waiting for its number, if any. Anything else is someone else's, and left alone.
     */
    void received(String data) {
        String hex = data == null ? "" : data.replaceAll("[^0-9a-fA-F]", "").toUpperCase(Locale.ROOT);
        if (!hex.startsWith(SYSEX_START) || !hex.endsWith("F7") || hex.length() != SYSEX_START.length() + 8) return;
        int[] b = new int[3];
        for (int i = 0; i < 3; i++) {
            int start = SYSEX_START.length() + 2 * i;
            b[i] = Integer.parseInt(hex.substring(start, start + 2), 16);
            if (b[i] > 0x7F) return;
        }
        loopback = true;
        Sent sent = tokens.remove((b[0] << 14) | (b[1] << 7) | b[2]);
        if (sent != null) run(sent);
    }

    /**
     * A press of one of the link's buttons, which came back through the loopback: for the "button" route, Bitwig has run
     * the command bound to it already, and for "buttonCode" the link runs it now. Either way only for a press the link is
     * waiting for: it can't tell a late press from a new one of the same button, but drops what it waited for in vain.
     */
    private void pressed(String id, String route) {
        loopback = true;
        Sent sent = presses.remove(route + " " + id);
        if (sent == null || sent.dropped) return;
        if (route.equals("button")) sent.ran = true;
        else run(sent);
    }

    /** A command on its way by a route: which, its button's ID, what runs it, whether it ran, and whether the link gave up on it. */
    private static final class Sent {
        final String route;
        final String id;
        final Runnable run;
        boolean ran;
        boolean dropped;

        Sent(String route, String id, Runnable run) {
            this.route = route;
            this.id = id;
            this.run = run;
        }
    }

    /** The name of the project Bitwig has in front, or null when the link can't read it. */
    private String frontProject() {
        try {
            return str(app.projectName().get());
        } catch (RuntimeException e) {
            return null;
        }
    }

    /** The parameter last clicked in Bitwig (link version 3), with what Bitwig says of it. */
    private void clicked(Ask ask, Map<String, Object> request) {
        if (lastClicked == null)
            throw new LinkError("This Bitwig didn't let the link follow the last clicked parameter. A selftest says why.");
        Parameter p = clickedParameter;
        Map<String, Object> m = new LinkedHashMap<>();
        boolean exists = p.exists().get();
        m.put("exists", exists);
        if (exists) {
            m.put("name", str(p.name().get()));
            m.put("value", p.get());
            m.put("display", str(p.displayedValue().get()));
            addDetails(m, p);
        }
        m.put("locked", lastClicked.isLocked().get());
        reply(ask, m, null);
    }

    /**
     * Sets up a cursor clip for notes: its grid from the first step and the lowest key, what's read of it marked as
     * interesting, and the notes Bitwig shows in its grid kept as they come.
     */
    private NoteClip noteClip(String from, Clip clip) {
        clip.exists().markInterested();
        clip.getLoopStart().markInterested();
        clip.getLoopLength().markInterested();
        Track on = clip.getTrack();
        on.name().markInterested();
        BooleanValue selected = null;
        if (cursorTrack != null) {
            // Only for onSelectedTrack, so notes still works when Bitwig won't compare the clip's track.
            try {
                selected = on.createEqualsValue(cursorTrack);
                selected.markInterested();
            } catch (RuntimeException | LinkageError e) {
                selected = null;
            }
        }
        NoteClip made = new NoteClip(from, clip, on, selected);
        clip.setStepSize(NOTE_STEP);
        clip.scrollToKey(0);
        clip.scrollToStep(0);
        // Bitwig tells each step of the grid that changes, for each channel: where a note starts (NoteOn), where one goes
        // on (NoteSustain), and where there's none (Empty). Only the starts are kept.
        clip.addNoteStepObserver(step -> {
            List<Integer> at = List.of(step.channel(), step.x(), step.y());
            switch (step.state()) {
                case NoteOn -> made.notes.put(at, new GridNote(step.x(), step.y(), step.duration(), step.velocity()));
                default -> made.notes.remove(at);
            }
        });
        return made;
    }

    /**
     * The notes of the clip selected in the arranger and of the one selected in the clip launcher (extension 3.15), for
     * the Playground's Get chords from Bitwig: for each clip there is (the arranger's first), where it's `from`, its
     * `track` and whether that's the selected one, its loop `start` and `length` in beats, whether it's longer than the
     * grid the link reads it through (`cut`), and its `notes` (`start` in beats from the clip's start, `length` in beats,
     * `key` and `velocity`, 0 to 127), by start, then key.
     */
    private void notes(Ask ask, Map<String, Object> request) {
        if (noteClips == null) throw new LinkError("This Bitwig didn't let the link read its clips' notes. A selftest says why.");
        List<Object> clips = new ArrayList<>();
        for (NoteClip c : noteClips) {
            if (!c.clip.exists().get()) continue;
            Map<String, Object> m = new LinkedHashMap<>();
            m.put("from", c.from);
            m.put("track", str(c.track.name().get()));
            m.put("onSelectedTrack", c.selected != null && c.selected.get());
            double start = c.clip.getLoopStart().get();
            double length = c.clip.getLoopLength().get();
            m.put("start", start);
            m.put("length", length);
            m.put("cut", start + length > NOTE_STEPS * NOTE_STEP + 1e-9);
            List<GridNote> sorted = new ArrayList<>(c.notes.values());
            sorted.sort(Comparator.comparingInt(GridNote::step).thenComparingInt(GridNote::key));
            List<Object> notes = new ArrayList<>();
            for (GridNote n : sorted) {
                Map<String, Object> note = new LinkedHashMap<>();
                note.put("start", n.step() * NOTE_STEP);
                note.put("length", n.length());
                note.put("key", n.key());
                note.put("velocity", (int) Math.max(0, Math.min(127, Math.round(n.velocity() * 127))));
                notes.add(note);
            }
            m.put("notes", notes);
            clips.add(m);
        }
        if (clips.isEmpty()) throw new LinkError(NO_CLIP);
        Map<String, Object> m = new LinkedHashMap<>();
        m.put("clips", clips);
        reply(ask, m, null);
    }

    /** Runs a self-check, writes it to the reports folder and returns it, with the file it went in. */
    private Map<String, Object> selfTest(String why) {
        Map<String, Object> report = selfCheck(why);
        Path file = reports.writeSelfCheck(report);
        report.put("file", file == null ? null : file.toString());
        return report;
    }

    // ---- Changes that wait for Bitwig ----

    /**
     * Inserts something into a device chain: a plugin by its ID (as the app's plugin scanner reads it), a Bitwig
     * device by its ID, or a file (a preset, or a sample, which Bitwig loads into a Sampler). It goes at the end
     * of the selected track's chain, or of `track`'s (at its start with `at: "start"`). With a track, the answer
     * waits to say whether Bitwig added a device, since it quietly ignores what it can't use.
     */
    private void insert(Map<String, Object> request, Finish finish) {
        Object t = field(request, "track");
        Object at = field(request, "at");
        boolean start = "start".equals(at);
        if (!absent(at) && !start && !"end".equals(at)) throw new LinkError("Insert at the \"start\" or the \"end\" of the chain.");
        InsertionPoint point;
        DeviceBank chain = null;
        int before = 0;
        if (absent(t)) {
            if (start) throw new LinkError("Say which track's chain to insert at the start of.");
            if (!cursorTrack.exists().get()) throw new LinkError("No track is selected in Bitwig.");
            point = cursorEnd;
        } else {
            int track = checkTrack(t);
            chain = trackDevices[track];
            before = chain.itemCount().get();
            point = start ? trackStarts[track] : trackEnds[track];
        }
        Object kind = field(request, "kind");
        switch (kind instanceof String ? (String) kind : "\u0000") {
            case "vst3":
                point.insertVST3Device(given(field(request, "plugin"), "plugin ID"));
                break;
            case "clap":
                point.insertCLAPDevice(given(field(request, "plugin"), "plugin ID"));
                break;
            case "vst2":
                point.insertVST2Device(vst2Id(field(request, "plugin")));
                break;
            case "bitwig":
                point.insertBitwigDevice(uuid(given(field(request, "device"), "device ID")));
                break;
            case "file":
                point.insertFile(given(field(request, "path"), "file"));
                break;
            default:
                throw new LinkError("Can't insert a \"" + Json.js(kind) + "\": use vst3, clap, vst2, bitwig or file.");
        }
        if (chain == null) {
            // Bitwig quietly does nothing for an ID it doesn't know; the app checks the device list afterwards.
            finish.ok(Map.of());
            return;
        }
        DeviceBank devices = chain;
        int count = before;
        int index = start ? 0 : before;
        waitFor(() -> devices.itemCount().get() > count, CHANGE_WAIT, added -> {
            if (!added) {
                Map<String, Object> m = new LinkedHashMap<>();
                m.put("added", false);
                m.put("index", null);
                m.put("device", null);
                finish.ok(m);
                return;
            }
            settle(() -> index < MAX_DEVICES && ((Device) devices.getItemAt(index)).exists().get()
                         ? describeDevice(index, (Device) devices.getItemAt(index)) : null,
                   device -> {
                       Map<String, Object> m = new LinkedHashMap<>();
                       m.put("added", true);
                       m.put("index", index);
                       m.put("device", device);
                       finish.ok(m);
                   }, finish);
        }, finish);
    }

    /**
     * Adds an instrument track at the end of the project's tracks (before the effect tracks), names it and
     * selects it, and answers with its index in the project listing, and (extension 3.9) its ID when Bitwig has said it.
     */
    private void newTrack(Map<String, Object> request, Finish finish) {
        Object given = field(request, "name");
        if (!absent(given) && !(given instanceof String)) throw new LinkError("A track's name is text.");
        String name = absent(given) ? "" : (String) given;
        Object colour = field(request, "color");
        float[] rgb = absent(colour) ? null : parseColour(colour);
        int count = tracks.itemCount().get();
        List<String> before = trackList();
        app.createInstrumentTrack(-1);
        // The new track shows in the listing where the tracks first differ: Bitwig adds it after the last main
        // track, so the effect tracks and the master move down one.
        waitFor(() -> tracks.itemCount().get() > count && firstDifference(before, trackList()) >= 0, CHANGE_WAIT, shown -> {
            if (!shown && tracks.itemCount().get() > count)
                throw new LinkError("Bitwig added the track past the first " + MAX_TRACKS + " tracks, where the app can't reach it.");
            if (!shown) throw new LinkError("Bitwig didn't add the track.");
            settle(this::trackList, after -> {
                int t = firstDifference(before, after);
                if (t < 0 || !trackItems[t].exists().get()) throw new LinkError("Bitwig didn't add the track.");
                Track track = trackItems[t];
                // In case adding a track opened Bitwig's browser to pick an instrument: the app brings its own.
                if (popupBrowser.exists().get()) popupBrowser.cancel();
                if (!name.isEmpty()) track.name().set(name);
                if (rgb != null) track.color().set(rgb[0], rgb[1], rgb[2]);
                cursorTrack.selectChannel(track);
                waitFor(() -> name.isEmpty() || str(track.name().get()).equals(name), 1000, named -> {
                    Map<String, Object> m = new LinkedHashMap<>();
                    m.put("index", t);
                    m.put("name", str(track.name().get()));
                    // Extension 3.9: the ID Bitwig keeps it under, when it has said, which finds it however tracks move.
                    String id = idOf(t);
                    if (!id.isEmpty()) m.put("id", id);
                    finish.ok(m);
                }, finish);
            }, finish);
        }, finish);
    }

    /**
     * Puts a file (a MIDI file, or audio) in one of a track's clip launcher slots: the one given, or the first
     * empty one, and answers once Bitwig has made the clip. The track is `track`, or (extension 3.9) the one with ID `trackId`.
     */
    private void addClip(Map<String, Object> request, Finish finish) {
        int t = requestedTrack(request);
        String path = given(field(request, "path"), "file");
        boolean replace = flag(request, "replace", false);
        boolean launch = flag(request, "launch", false);
        if (replace && absent(field(request, "slot"))) throw new LinkError("Say which slot's clip to replace.");
        ClipLauncherSlot[] slots = trackSlots[t];
        Object s = field(request, "slot");
        int index;
        if (!absent(s)) {
            if (!isIndex(s, SCENES))
                throw new LinkError("The app can reach the first " + SCENES + " clip slots on a track, not slot " + Json.js(s) + ".");
            index = index(s);
            if (slots[index].hasContent().get() && !replace) throw new LinkError("Slot " + index + " on that track already has a clip.");
        } else {
            index = emptySlot(slots);
            if (index < 0) throw new LinkError("The track's first " + SCENES + " clip slots are full.");
        }
        // A replaced clip is there all along, so it's the clip's name that changes (the app names each file it swaps in
        // differently from the one before, extension 3.17).
        String before = replace ? str(slots[index].name().get()) : "";
        // Only a real swap waits the shorter time: a replace into an empty slot (a new Playground track's first part) is
        // a new clip, which Bitwig may take as long to make as any other (extension 3.18).
        boolean swap = replace && slots[index].hasContent().get();
        slotInserts[t][index].insertFile(path);
        // The track's slot, found by its ID when Bitwig has said it, in case a track goes in above it meanwhile.
        String id = idOf(t);
        waitFor(() -> {
            int now = id.isEmpty() ? t : indexOf(id);
            return now >= 0 && trackSlots[now][index].hasContent().get()
                   && (!replace || !str(trackSlots[now][index].name().get()).equals(before));
        }, swap ? REPLACE_WAIT : CHANGE_WAIT, made -> {
            int now = id.isEmpty() ? t : indexOf(id);
            // A replace whose name didn't change (the same file again) still has its clip: it goes ahead.
            if (!made && !(replace && now >= 0 && trackSlots[now][index].hasContent().get()))
                throw new LinkError("Bitwig didn't make a clip from " + path + ".");
            Map<String, Object> m = new LinkedHashMap<>();
            m.put("slot", index);
            if (launch) {
                if (now < 0) throw new LinkError("The track went before its clip could be launched.");
                // In step with the song (extension 3.17): a swapped clip carries on from where Bitwig is, not from its own
                // start, so it stays with the parts that didn't change.
                trackSlots[now][index].launchWithOptions("default", "synced");
                m.put("launched", true);
            }
            finish.ok(m);
        }, finish);
    }

    /**
     * Stops the launcher clips playing on the tracks with the IDs given (`trackIds`) and puts them back on the arranger
     * (extension 3.16), for the Playground when it stops playing in Bitwig. A track that's gone is left out of `stopped`.
     */
    private void stopTracks(Map<String, Object> request, Finish finish) {
        Object given = field(request, "trackIds");
        if (!(given instanceof List)) throw new LinkError("Say which tracks to stop, by their IDs.");
        List<Object> stopped = new ArrayList<>();
        for (Object id : (List<?>) given) {
            if (!(id instanceof String)) throw new LinkError("A track's ID is text.");
            int t = indexOf((String) id);
            if (t < 0) continue;
            trackItems[t].stop();
            trackItems[t].returnToArrangement();
            stopped.add(id);
        }
        Map<String, Object> m = new LinkedHashMap<>();
        m.put("stopped", stopped);
        finish.ok(m);
    }

    /**
     * Whether Bitwig is `playing`, where (`position`, in beats from the start), where Play starts (`playStart`), its `tempo`
     * and `timeSignature` (extension 3.16), for the Playground's Follow Bitwig, which asks many times a second.
     */
    private void transportNow(Ask ask, Map<String, Object> request) {
        if (!followed) throw new LinkError("This Bitwig didn't let the link follow where it's playing. A selftest says why.");
        Map<String, Object> m = new LinkedHashMap<>();
        m.put("playing", transport.isPlaying().get());
        m.put("position", transport.playPosition().get());
        m.put("playStart", transport.playStartPosition().get());
        m.put("tempo", transport.tempo().getRaw());
        m.put("timeSignature", str(transport.timeSignature().get()));
        reply(ask, m, null);
    }

    /** The first empty slot in a scene that exists, else the first of the scenes still to come; -1 if none. */
    private static int emptySlot(ClipLauncherSlot[] slots) {
        for (int s = 0; s < slots.length; s++)
            if (slots[s].exists().get() && !slots[s].hasContent().get()) return s;
        for (int n = 0; n < slots.length; n++)
            if (!slots[n].exists().get()) return n;
        return -1;
    }

    /**
     * Puts samples on a Drum Machine's free pads (as if each were dropped on its pad, so each gets a Sampler),
     * leaving pads that have something on them alone. A pad that holds only what a Drum Machine preset put there,
     * a Sampler with no sample, is free too: the sample goes in that Sampler's place, keeping what comes after it.
     * The Drum Machine is `device` on `track` (which this selects), or the selected device.
     */
    private void fillPads(Map<String, Object> request, Finish finish) {
        Object given = field(request, "pads");
        if (!(given instanceof List) || ((List<?>) given).isEmpty()) throw new LinkError("No pads given.");
        List<Pad> list = new ArrayList<>();
        Set<Integer> taken = new HashSet<>();
        for (Object item : (List<?>) given) {
            Map<String, Object> pad = asMap(item);
            Object note = pad == null ? Json.UNDEFINED : field(pad, "note");
            if (pad == null || !isIndex(note, DRUM_PADS)) throw new LinkError("A pad's note is a MIDI note from 0 to 127.");
            if (!taken.add(index(note))) throw new LinkError("Two samples for note " + index(note) + ".");
            String path = given(field(pad, "path"), "sample");
            Object name = field(pad, "name");
            if (!absent(name) && !(name instanceof String)) throw new LinkError("A pad's name is text.");
            list.add(new Pad(index(note), path, absent(name) ? "" : (String) name));
        }
        Object t = field(request, "track");
        Object dGiven = field(request, "device");
        Object d = absent(dGiven) ? (Object) 0.0 : dGiven;
        boolean onTrack = !absent(t);
        int track = -1;
        int position = -1;
        if (onTrack) {
            track = checkTrack(t);
            Device device = trackDevice(track, d);
            position = index(d);
            cursorTrack.selectChannel(trackItems[track]);
            cursorDevice.selectDevice(device);
        } else if (!cursorDevice.exists().get()) {
            throw new LinkError("No device is selected in Bitwig. Select a Drum Machine and send the kit again.");
        }
        int ti = track;
        int di = position;
        waitFor(() -> cursorDevice.exists().get() && cursorDevice.hasDrumPads().get()
                      && (!onTrack || (trackSelected[ti].get() && cursorDevice.position().get() == di)),
                onTrack ? CHANGE_WAIT : 500, ready -> {
            if (!ready)
                throw new LinkError(onTrack ? "Device " + di + " on track " + ti + " isn't a Drum Machine."
                                            : "The selected device isn't a Drum Machine. Select one and send the kit again.");
            drumPads.scrollPosition().set(0);
            waitFor(() -> drumPads.scrollPosition().get() == 0, 2000, scrolled -> {
                if (!scrolled) throw new LinkError("Bitwig didn't show the Drum Machine's pads from the lowest note.");
                // The pads catch up with the selection a moment after it changes.
                settle(this::padList, pads -> fill(list, finish), finish);
            }, finish);
        }, finish);
    }

    /**
     * Drops each sample on its free pad, or in place of the pad's empty Sampler, then waits for the pads to take
     * them and names them. The answer says which samples went in place of an empty Sampler, and what's on each
     * pad it leaves alone.
     */
    private void fill(List<Pad> list, Finish finish) {
        List<Pad> filled = new ArrayList<>();
        Map<Integer, String> replaced = new LinkedHashMap<>();   // note -> the pad's name before
        List<Integer> skipped = new ArrayList<>();
        List<Map<String, Object>> taken = new ArrayList<>();
        for (Pad pad : list) {
            int n = pad.note();
            if (!padTaken(n)) {
                padInserts[n].insertFile(pad.path());
            } else if (emptySampler(n)) {
                replaced.put(n, str(padItems[n].name().get()));
                padReplace[n].insertFile(pad.path());
            } else {
                skipped.add(n);
                taken.add(padContents(n));
                continue;
            }
            filled.add(pad);
        }
        Predicate<Pad> took = pad -> replaced.containsKey(pad.note())
            ? !str(padFirst[pad.note()].sampleName().get()).isEmpty() || !str(padItems[pad.note()].name().get()).equals(replaced.get(pad.note()))
            : padTaken(pad.note());
        waitFor(() -> filled.stream().allMatch(took), CHANGE_WAIT, all -> {
            List<Integer> done = new ArrayList<>();
            List<Integer> failed = new ArrayList<>();
            for (Pad pad : filled) {
                if (!took.test(pad)) {
                    failed.add(pad.note());
                    continue;
                }
                if (!pad.name().isEmpty()) padItems[pad.note()].name().set(pad.name());
                done.add(pad.note());
            }
            Map<String, Object> m = new LinkedHashMap<>();
            m.put("filled", done);
            m.put("replaced", done.stream().filter(replaced::containsKey).collect(Collectors.toList()));
            m.put("skipped", skipped);
            m.put("failed", failed);
            m.put("taken", taken);
            finish.ok(m);
        }, finish);
    }

    /** Whether a pad has a device on it: the pad, its device count and its first device all show it. */
    private boolean padTaken(int note) {
        return padItems[note].exists().get() && padDevices[note].itemCount().get() > 0 && padFirst[note].exists().get();
    }

    /**
     * Whether a pad holds a Sampler with no sample, as a Drum Machine preset can put on every pad (Solomon's
     * default Drum Machine does), still has the name it came with, not a sample's, and has nothing after it but
     * effects: such a pad is free.
     */
    private boolean emptySampler(int note) {
        String first = str(padFirst[note].name().get());
        String name = str(padItems[note].name().get());
        return padTaken(note) && first.equals("Sampler") && str(padFirst[note].sampleName().get()).isEmpty()
               && (name.isEmpty() || name.equals(first)) && onlyEffectsAfter(note);
    }

    /**
     * Whether all a pad has after its first device are audio effects, so nothing else on it makes a sound. A pad with
     * more devices than the link sees ({@link #PAD_DEVICES}) doesn't count.
     */
    private boolean onlyEffectsAfter(int note) {
        int count = padDevices[note].itemCount().get();
        if (count > PAD_DEVICES) return false;
        for (int d = 1; d < count; d++) {
            Device after = (Device) padDevices[note].getItemAt(d);
            if (after.exists().get() && !audioEffect(str(after.deviceType().get()))) return false;
        }
        return true;
    }

    /**
     * Whether Bitwig's word for a device's type is an audio effect's. Bitwig 6.1.5 says "instrument" and "note-effect"
     * (the 2026-10-19 live run), so most likely "audio-effect"; "audio_to_audio", the word the link was first written
     * for, still counts.
     */
    static boolean audioEffect(String type) {
        return type.equals("audio-effect") || type.equals("audio_to_audio");
    }

    /** What's on a pad: its name, how many devices, the first one's name and its sample, as Bitwig shows them. */
    private Map<String, Object> padContents(int note) {
        Map<String, Object> m = new LinkedHashMap<>();
        m.put("note", note);
        m.put("name", str(padItems[note].name().get()));
        m.put("devices", padDevices[note].itemCount().get());
        m.put("first", str(padFirst[note].name().get()));
        m.put("sample", str(padFirst[note].sampleName().get()));
        return m;
    }

    /**
     * Sets the pitch of a Sampler (its Transpose, in semitones, which can be a fraction) or a Note Transpose
     * (whole semitones) among the first devices on a track, without selecting it, and answers with what Bitwig
     * shows.
     */
    private void transpose(Map<String, Object> request, Finish finish) {
        int t = checkTrack(field(request, "track"));
        Object d = field(request, "device");
        if (!isIndex(d, TUNED_DEVICES))
            throw new LinkError("The app can set the pitch of the first " + TUNED_DEVICES + " devices on a track, not device " + Json.js(d) + ".");
        Device device = trackDevice(t, d);
        Object given = field(request, "semitones");
        if (!(given instanceof Double) || !Double.isFinite((Double) given)) throw new LinkError("Say how many semitones.");
        double semitones = (Double) given;
        int di = index(d);
        Tuning tunings = trackTunings[t][di];
        // A device that was just added is known by its ID a moment after it appears.
        waitFor(() -> tunings.sampler().exists().get() || tunings.transpose().exists().get(), 2000, known -> {
            if (!known) throw new LinkError("Device " + di + " on track " + t + " is neither a Sampler nor a Note Transpose.");
            Parameter param = tunings.sampler().exists().get() ? tunings.sampler() : tunings.transpose();
            param.setRaw(semitones);
            waitFor(() -> Math.abs(param.getRaw() - semitones) < 0.001, 1000, set -> {
                Map<String, Object> m = new LinkedHashMap<>();
                m.put("device", str(device.name().get()));
                m.put("value", param.getRaw());
                m.put("display", str(param.displayedValue().get()));
                finish.ok(m);
            }, finish);
        }, finish);
    }

    /**
     * Starts or stops Bitwig's master recorder, or says what it's doing (link version 3): `do` is "start", "stop"
     * or "status" (the default). Answers with `active` and `duration` (how long it has recorded, in ms).
     */
    private void useRecorder(Map<String, Object> request, Finish finish) {
        if (recorder == null)
            throw new LinkError("This Bitwig didn't let the link use its master recorder. A selftest says why.");
        Object what = field(request, "do");
        String action = absent(what) ? "status" : what instanceof String ? (String) what : "";
        switch (action) {
            case "status":
                finish.ok(recorderState());
                break;
            case "start":
            case "stop": {
                boolean on = action.equals("start");
                if (recorder.isActive().get() != on) {
                    if (on) recorder.start();
                    else recorder.stop();
                }
                waitFor(() -> recorder.isActive().get() == on, CHANGE_WAIT, done -> {
                    if (!done) throw new LinkError(on ? "Bitwig didn't start recording. Is its audio engine on?" : "Bitwig didn't stop recording.");
                    finish.ok(recorderState());
                }, finish);
                break;
            }
            default:
                throw new LinkError("Tell the recorder to \"start\", \"stop\" or give its \"status\", not " + Json.js(what) + ".");
        }
    }

    private Map<String, Object> recorderState() {
        Map<String, Object> m = new LinkedHashMap<>();
        m.put("active", recorder.isActive().get());
        m.put("duration", recorder.duration().get());
        return m;
    }

    /**
     * An experiment (extension 3.6): puts a copy of a launcher clip in Bitwig's arranger, on the clip's own track, `at`
     * beats from the start (where the play start is, unless given). Bitwig's API has no way to put a clip in the arranger
     * (only launcher slots have insertion points), so the link does what a person would: show the Arrange layout with its
     * clip launcher, select the clip, give the launcher the focus, copy, move the play start there, bring the arranger
     * timeline to the front and paste. Bitwig copies and pastes in whichever of its panels has the focus, which the API
     * can't read, so:
     * <ul>
     * <li>the link gives the clip launcher the focus with Bitwig's FOCUS_LAUNCHER action when Bitwig has one, and copies
     * from whichever panel has the focus when it hasn't (`launcherFocus` says how many times it ran, 0 without one);</li>
     * <li>when FOCUS_ARRANGER hid the arranger timeline rather than bringing it to the front (`focus` 2), the timeline had
     * the focus when the link copied, as nothing in between moves it. Bitwig copied from the timeline, not the launcher
     * clip, so the link doesn't paste, and says `copiedFromTimeline`.</li>
     * </ul>
     * Paste puts down whatever is on Bitwig's clipboard, which every open project shares. When the copy didn't take, that's
     * whatever was copied before: a clip copied in someone's own song, or the last run's copy. So before it copies, the
     * link marks the launcher clip with a fresh colour (`mark`; only test projects get this, so changing the clip's colour
     * is fine), and a clip pasted with that colour can be told from whatever Bitwig's clipboard held before.
     *
     * Nobody knows yet whether Bitwig does all that for an extension, so once the steps are through, the answer says what
     * Bitwig showed before and after, whether or not it worked: `appeared` is whether the clip selected in the arranger
     * moved to the track after the paste, with the track's launcher clips as they were; `sameAsSource` is whether that clip
     * has the launcher clip's marking colour (`pastedColor` is its colour); and `arranged` is whether both are true and the
     * launcher clip got the very colour the link asked for (the mark is `exact`). When Bitwig rounded the mark to one of
     * its own colours, other clips can have that colour too (one already on Bitwig's clipboard, say), so a pasted clip in
     * it may not be the launcher clip's copy. Where the clip starts can't be read, so only a look at Bitwig shows whether
     * it went in at `at`. Only the live tests send it, in their own project, so nothing is undone when it doesn't work.
     *
     * Extension 3.7: the request can ask for a route (`via`, see ROUTES) for the copy, the paste and the two focus actions.
     * "direct" runs them as before, with the API's copy and paste. Any other route runs Bitwig's Copy and Paste actions and
     * the focus actions by that route; without one, that's DEFAULT_ROUTE (from 3.8; "direct" before). `ran` says, for
     * each of those steps, whether it ran (null when the link didn't get that far, or Bitwig has no FOCUS_LAUNCHER). A step
     * that didn't run doesn't stop the change with an error: the link answers with what Bitwig shows, and doesn't paste
     * after a copy that didn't run.
     *
     * Extension 3.8: with `checks` true, when arrange's paste ran but put nothing in the arranger that the link can see, and
     * Bitwig has FOCUS_LAUNCHER (see worthChecking), two more steps in the launcher say which step Bitwig dropped, a paste
     * into an empty slot and a Duplicate, before the answer (see check).
     */
    private void arrange(Map<String, Object> request, Finish asked) {
        String route = route(request);
        if (arranger == null) throw new LinkError("This Bitwig didn't let the link reach its arranger. A selftest says why.");
        Object c = field(request, "checks");
        if (!absent(c) && !(c instanceof Boolean)) throw new LinkError("checks is true or false, not " + Json.js(c) + ".");
        boolean checks = Boolean.TRUE.equals(c);
        int t = checkTrack(field(request, "track"));
        Object s = field(request, "slot");
        if (!isIndex(s, SCENES))
            throw new LinkError("The app can reach the first " + SCENES + " clip slots on a track, not slot " + Json.js(s) + ".");
        int index = index(s);
        Object given = field(request, "at");
        if (!absent(given) && !(given instanceof Double && Double.isFinite((Double) given) && (Double) given >= 0))
            throw new LinkError("Say where in the arranger as beats from the start (0 or more), not " + Json.js(given) + ".");
        ClipLauncherSlot slot = trackSlots[t][index];
        if (!slot.hasContent().get()) throw new LinkError("Slot " + index + " on that track has no clip to put in the arranger.");
        Action focus = app.getAction(FOCUS_ARRANGER);
        if (focus == null)
            throw new LinkError("Bitwig has no action \"" + FOCUS_ARRANGER + "\", which the link runs to bring the arranger timeline to the front.");
        Action launcherFocus = app.getAction(FOCUS_LAUNCHER);
        // What copies and pastes: the API's copy and paste straight away, Bitwig's Copy and Paste actions by any other route.
        Runnable copyRun = app::copy;
        Runnable pasteRun = app::paste;
        if (!route.equals(DIRECT)) {
            Action copyAction = app.getAction("Copy");
            Action pasteAction = app.getAction("Paste");
            if (copyAction == null || pasteAction == null)
                throw new LinkError("Bitwig has no \"Copy\" and \"Paste\" actions, which the link runs to copy and paste via " + route + ".");
            copyRun = copyAction::invoke;
            pasteRun = pasteAction::invoke;
            List<String> needed = new ArrayList<>(List.of("Copy", "Paste", FOCUS_ARRANGER));
            if (launcherFocus != null) needed.add(FOCUS_LAUNCHER);
            checkButtons(route, needed);
        }
        Runnable copied = copyRun;
        Runnable pasted = pasteRun;
        Action duplicate = checks ? app.getAction("Duplicate") : null;
        if (duplicate != null && !route.equals(DIRECT)) checkButtons(route, List.of("Duplicate"));
        double at = absent(given) ? transport.playStartPosition().get() : (Double) given;
        Track track = trackItems[t];
        String name = str(track.name().get());
        // So the checks delete only on this track, should another go in above it (extension 3.9).
        String trackId = idOf(t);
        SettableBooleanValue launcherShown = arranger.isClipLauncherVisible();

        // The answer, in the order the app reads it, filled in as the steps go: first what Bitwig shows before.
        Shown shown = new Shown(launcherShown.get(), arranger.isTimelineVisible().get());
        Map<String, Object> m = new LinkedHashMap<>();
        m.put("arranged", false);
        m.put("appeared", false);
        m.put("sameAsSource", false);
        m.put("copiedFromTimeline", false);
        m.put("at", at);
        m.put("layout", str(app.panelLayout().get()));
        m.put("launcher", shown.launcher);
        m.put("mark", null);
        m.put("launcherFocus", 0);
        m.put("timeline", shown.timeline);
        m.put("focus", 0);
        m.put("before", arrangerClipState());
        m.put("beforePaste", null);
        m.put("after", null);
        m.put("pastedColor", null);
        int launcherBefore = launcherClips(t);
        m.put("launcherClipsBefore", launcherBefore);
        m.put("launcherClipsAfter", null);
        m.put("via", route);
        Map<String, Object> ran = new LinkedHashMap<>();
        ran.put("launcherFocus", null);
        ran.put("copy", null);
        ran.put("focus", null);
        ran.put("paste", null);
        m.put("ran", ran);
        // With checks asked for, the answer waits for them when they can tell something (see worthChecking and check).
        // They need the mark the clip got, once it has one.
        Mark[] marked = new Mark[1];
        Finish finish = !checks ? asked : (result, error) -> {
            if (error != null || !worthChecking(m, launcherFocus, name)) asked.done(result, error);
            else check(route, t, trackId, index, track, launcherFocus, launcherShown, pasted, duplicate, marked[0], m, asked);
        };

        // The Arrange layout, with its clip launcher showing.
        if (!arrangeLayout()) app.setPanelLayout(Application.PANEL_LAYOUT_ARRANGE);
        waitFor(this::arrangeLayout, CHANGE_WAIT, arranging -> {
            if (!arranging) throw new LinkError("Bitwig didn't switch to its Arrange layout.");
            if (!launcherShown.get()) launcherShown.set(true);
            waitFor(launcherShown::get, CHANGE_WAIT, showing -> {
                if (!showing) throw new LinkError("Bitwig didn't show the clip launcher in its Arrange layout.");
                // The clip, selected in the launcher, which has the focus as the link copies (as far as it can tell).
                cursorTrack.selectChannel(track);
                slot.select();
                waitFor(() -> trackSelected[t].get() && slot.isSelected().get(), CHANGE_WAIT, selected -> {
                    if (!selected) throw new LinkError("Bitwig didn't select the clip in slot " + index + " of track " + t + ".");
                    // The clip in its marking colour, so the link can tell it once it's pasted.
                    mark(slot, m, mark -> {
                        marked[0] = mark;
                        Runnable copy = () -> runBy(route, "Copy", copied, copyRan -> {
                            ran.put("copy", copyRan);
                            if (!copyRan) {
                                // Nothing new on Bitwig's clipboard, so nothing to paste.
                                m.put("beforePaste", arrangerClipState());
                                answer(t, m, shown, finish);
                                return;
                            }
                            transport.playStartPosition().set(at);
                            waitFor(() -> Math.abs(transport.playStartPosition().get() - at) < 1e-6, CHANGE_WAIT, moved -> {
                                if (!moved) throw new LinkError("Bitwig didn't move the play start to beat " + Json.number(at) + ".");
                                focusPanel(route, FOCUS_ARRANGER, focus, arranger.isTimelineVisible(), "arranger timeline", m, "focus", ran, times -> {
                                    if (times == 1) {
                                        paste(route, pasted, ran, t, name, launcherBefore, mark, m, shown, finish);
                                        return;
                                    }
                                    // It hid the timeline, which only happens when the timeline is in front: so it had the
                                    // focus as the link copied, and Bitwig's clipboard doesn't hold the launcher clip.
                                    // (Or the focus action didn't run, so the timeline may not have the focus to paste in.)
                                    if (times == 2) m.put("copiedFromTimeline", true);
                                    m.put("beforePaste", arrangerClipState());
                                    answer(t, m, shown, finish);
                                }, finish);
                            }, finish);
                        }, finish);
                        if (launcherFocus == null) copy.run();
                        else focusPanel(route, FOCUS_LAUNCHER, launcherFocus, launcherShown, "clip launcher", m, "launcherFocus", ran,
                                        times -> copy.run(), finish);
                    }, finish);
                }, finish);
            }, finish);
        }, finish);
    }

    /** Whether Bitwig showed its clip launcher (in the Arrange layout) and arranger timeline, before and after. */
    private static final class Shown {
        final Map<String, Object> launcher = new LinkedHashMap<>();
        final Map<String, Object> timeline = new LinkedHashMap<>();

        Shown(boolean launcherBefore, boolean timelineBefore) {
            launcher.put("before", launcherBefore);
            launcher.put("after", null);
            timeline.put("before", timelineBefore);
            timeline.put("after", null);
        }
    }

    private boolean arrangeLayout() {
        return Application.PANEL_LAYOUT_ARRANGE.equals(str(app.panelLayout().get()));
    }

    /**
     * Brings one of Bitwig's panels (`what`, which `visible` says is shown or not) to the front with Bitwig's
     * focus-or-toggle `action` for it (its ID is `id`), run by `route`, then hands `then` what happened: 1 when the panel
     * is in front, 2 when the action hid it (it was in front already), and 0 when the route didn't run the action. The
     * answer keeps how many times the action ran under `key`, and `ran` whether the route ran it each time. Such an
     * action hides its panel when that's in front already, so when it does, the link runs it again to show the panel,
     * in front.
     */
    private void focusPanel(String route, String id, Action action, BooleanValue visible, String what, Map<String, Object> m, String key,
                            Map<String, Object> ran, IntConsumer then, Finish finish) {
        boolean shown = visible.get();
        runBy(route, id, action::invoke, first -> {
            ran.put(key, first);
            if (!first) {
                then.accept(0);
                return;
            }
            m.put(key, 1);
            // Hidden, the panel comes into view. Shown, it stays (now in front) unless it was in front already: then it hides,
            // which the link gives it a moment to do.
            waitFor(() -> visible.get() != shown, shown ? TOGGLE_WAIT : CHANGE_WAIT, changed -> {
                if (!shown && !changed) throw new LinkError("Bitwig didn't show its " + what + ".");
                if (!shown || !changed) {
                    then.accept(1);
                    return;
                }
                runBy(route, id, action::invoke, second -> {
                    ran.put(key, second);
                    if (!second) {
                        // Hidden it stays, and the link carries on all the same: the panel had been in front.
                        then.accept(2);
                        return;
                    }
                    m.put(key, 2);
                    waitFor(visible::get, CHANGE_WAIT, again -> {
                        if (!again) throw new LinkError("Bitwig hid its " + what + ", which was in front already, and didn't show it again.");
                        then.accept(2);
                    }, finish);
                }, finish);
            }, finish);
        }, finish);
    }

    /**
     * Marks a launcher clip with a fresh colour (see nextMark), so that a clip pasted from it can be told from whatever else
     * Bitwig's clipboard held, and hands `then` how that went, which the answer keeps as `mark`. Bitwig may round a colour
     * to one of its own, so the mark took when the clip shows the colour asked for, or at least a different one than before;
     * the colour read back then is what the pasted clip is compared with. The mark is exact when that's the colour asked
     * for (give or take SAME_COLOUR): a colour Bitwig rounded is one of its own, which other clips can have too, so only an
     * exact mark tells the launcher clip's copy from them for sure. When the clip's colour doesn't change, the link carries
     * on all the same, though it can't tell its own clip from another when it's pasted.
     */
    private void mark(ClipLauncherSlot slot, Map<String, Object> m, Consumer<Mark> then, Finish finish) {
        SettableColorValue colour = slot.color();
        float[] before = rgb(colour);
        float[] asked = nextMark(before);
        colour.set(asked[0], asked[1], asked[2]);
        waitFor(() -> sameColour(rgb(colour), asked) || !sameColour(rgb(colour), before), CHANGE_WAIT, took -> {
            float[] got = rgb(colour);
            Mark mark = new Mark(before, asked, got, took, sameColour(got, asked));
            m.put("mark", mark.answer());
            then.accept(mark);
        }, finish);
    }

    /**
     * A fresh colour to mark a launcher clip with: the next hue round the colour wheel from the last mark's (a GOLDEN step
     * on), fairly strong and bright so it stands out. While it looks like the clip's colour `now`, or the last mark, it
     * steps on again.
     */
    private float[] nextMark(float[] now) {
        float[] last = markColour(markHue);
        float[] mark;
        int tries = 0;
        do {
            markHue = (markHue + GOLDEN) % 1;
            mark = markColour(markHue);
        } while ((!clearlyDifferent(mark, now) || !clearlyDifferent(mark, last)) && ++tries < 8);
        return mark;
    }

    /** A marking colour of a hue (0 to 1, round the colour wheel): fairly strong (saturation 0.7) and bright (0.85). */
    static float[] markColour(double hue) {
        return hsb(hue, 0.7, 0.85);
    }

    /**
     * How arrange marked the launcher clip: its colour before, the one asked for, the one it got, whether it took (the
     * colour changed), and whether it's exact (the clip got the very colour asked for, not one Bitwig rounded it to).
     */
    private record Mark(float[] before, float[] asked, float[] got, boolean took, boolean exact) {
        Map<String, Object> answer() {
            Map<String, Object> m = new LinkedHashMap<>();
            m.put("before", hexColour(before));
            m.put("asked", hexColour(asked));
            m.put("got", hexColour(got));
            m.put("took", took);
            m.put("exact", exact);
            return m;
        }
    }

    /**
     * Pastes what the link copied, then waits for the clip selected in the arranger to show up on the track called `name`
     * (where it wasn't just before), or for the track's launcher clips to change (Bitwig pasted there instead), and
     * answers with what Bitwig shows once it settles. When neither happens, it answers after CHANGE_WAIT all the same.
     * A clip that appeared on the track is the launcher clip's copy only when it has the `mark` colour: else Bitwig pasted
     * whatever its clipboard held before. Even then, the link says it arranged the clip only when the mark is exact, as a
     * colour Bitwig rounded to one of its own could be another clip's as well.
     */
    private void paste(String route, Runnable pasteRun, Map<String, Object> ran, int t, String name, int launcherBefore, Mark mark,
                       Map<String, Object> m, Shown shown, Finish finish) {
        m.put("beforePaste", arrangerClipState());
        boolean there = clipOn(name);
        runBy(route, "Paste", pasteRun, pasted -> {
            ran.put("paste", pasted);
            if (!pasted) {
                answer(t, m, shown, finish);
                return;
            }
            waitFor(() -> (!there && clipOn(name)) || launcherClips(t) != launcherBefore, CHANGE_WAIT, seen -> {
                // Bitwig reports a change bit by bit, so what it did is read once it stops changing.
                settle(() -> Arrays.asList(arrangerClipState(), arrangerClipColour(), launcherClips(t)), settled -> {
                    boolean appeared = !there && clipOn(name) && launcherClips(t) == launcherBefore;
                    boolean sameAsSource = mark.took() && arrangerClip.exists().get() && sameColour(rgb(arrangerClip.color()), mark.got());
                    m.put("arranged", appeared && sameAsSource && mark.exact());
                    m.put("appeared", appeared);
                    m.put("sameAsSource", sameAsSource);
                    answer(t, m, shown, finish);
                }, finish);
            }, finish);
        }, finish);
    }

    /**
     * Whether arrange's checks can tell anything after it (`m` is its answer so far), without putting down what the link
     * can't take back: only when its own paste ran (so the copy ran, from the launcher, and the timeline was brought to the
     * front) and put nothing in the arranger (no clip appeared on the track, and the clip selected there is as it was),
     * and Bitwig has the launcher's focus action, which keeps the checks' paste and Duplicate in the launcher. Otherwise
     * a check's paste could put down what Bitwig's clipboard held before, or a clip in the arranger, which the link would
     * neither see nor delete, and would hide a clip a later go put there. Nor when a clip on the track (called `name`) was
     * selected in the arranger before the paste: the link tells clips there apart only by their track, so one the paste
     * put on that track would look just like it.
     */
    private static boolean worthChecking(Map<String, Object> m, Action launcherFocus, String name) {
        Map<?, ?> ran = (Map<?, ?>) m.get("ran");
        Object before = m.get("beforePaste");
        Object after = m.get("after");
        boolean unseen = before instanceof Map && name.equals(((Map<?, ?>) before).get("track"));
        boolean inArranger = Boolean.TRUE.equals(m.get("appeared"))
                || (after instanceof Map && Boolean.TRUE.equals(((Map<?, ?>) after).get("exists")) && !after.equals(before));
        return launcherFocus != null && Boolean.TRUE.equals(ran.get("paste")) && !inArranger && !unseen;
    }

    /**
     * Arrange's checks (extension 3.8), run when the request asks for them and arrange's paste ran but put nothing in the
     * arranger (see worthChecking): two more steps by the same route, each in the clip launcher, that tell which of
     * arrange's steps Bitwig dropped. Then it answers with `m`, which gets them as `checks`:
     * <ul>
     * <li>`launcherPaste`: the link selects an empty launcher slot on the track (the first that exists), gives the clip
     * launcher the focus as arrange does, and pastes. When the mark took and is exact, a clip in the mark's colour
     * appearing there (`made`, `colour`, `sameAsMark`) means Bitwig's Copy did copy the clip and its Paste works in the
     * launcher, so what went wrong was the arranger's part (a colour Bitwig rounded the mark to, a clip already on its
     * clipboard can have too). The clip it made is deleted again (`removed`), so the next go starts as this one did. Null
     * when the track has no empty slot the link can reach.</li>
     * <li>`duplicate`: the link selects the clip's slot again, gives the launcher the focus, and runs Bitwig's Duplicate
     * action, which needs no clipboard. A new launcher clip (`launcher`, how many more the track has) means Bitwig's
     * editing actions work in the launcher by this route; a new track (`tracks`) means Duplicate went to the track, so the
     * launcher didn't have the focus. A new launcher clip is deleted again (`removed`); a new track is left. Null when
     * Bitwig has no Duplicate action.</li>
     * </ul>
     * Each says whether its slot got `selected`, how many times the launcher's focus action ran (`focus`) and whether each
     * step `ran` by the route. A check stops before its paste or Duplicate when Bitwig doesn't select its slot or the
     * route doesn't run the launcher's focus action, since either could go to another panel then. A paste or Duplicate
     * that goes anywhere but the launcher (or a new track) all the same isn't seen, and is left. A check that can't go on
     * (Bitwig won't show the launcher, say) leaves the rest out and puts what stopped it in `checks` as `error`, and
     * arrange still answers with what it found.
     *
     * Extension 3.9: a check deletes only on the track Bitwig keeps under `id` (when it has said it): if another track has
     * taken its place in the listing meanwhile (a paste can put tracks in above it), the check leaves the clip and says
     * `trackMoved`.
     */
    private void check(String route, int t, String id, int index, Track track, Action launcherFocus, SettableBooleanValue launcherShown,
                       Runnable pasteRun, Action duplicate, Mark mark, Map<String, Object> m, Finish finish) {
        Map<String, Object> checks = new LinkedHashMap<>();
        checks.put("launcherPaste", null);
        checks.put("duplicate", null);
        m.put("checks", checks);
        Finish done = (result, error) -> {
            if (error != null) checks.put("error", error);
            finish.ok(m);
        };
        Runnable duplicating = () -> duplicateCheck(route, t, id, index, track, launcherFocus, launcherShown, duplicate, checks, done);
        ClipLauncherSlot[] slots = trackSlots[t];
        int empty = -1;
        for (int s = 0; s < slots.length && empty < 0; s++)
            if (slots[s].exists().get() && !slots[s].hasContent().get()) empty = s;
        if (empty < 0) {
            duplicating.run();
            return;
        }
        int e = empty;
        ClipLauncherSlot slot = slots[e];
        Map<String, Object> lp = new LinkedHashMap<>();
        Map<String, Object> ran = new LinkedHashMap<>();
        lp.put("slot", e);
        lp.put("selected", false);
        lp.put("focus", 0);
        ran.put("focus", null);
        ran.put("paste", null);
        lp.put("ran", ran);
        lp.put("made", false);
        lp.put("colour", null);
        lp.put("sameAsMark", false);
        lp.put("removed", false);
        checks.put("launcherPaste", lp);
        inLauncher(route, t, slot, track, launcherFocus, launcherShown, lp, ran, duplicating, () -> runBy(route, "Paste", pasteRun, pasted -> {
            ran.put("paste", pasted);
            if (!pasted) {
                duplicating.run();
                return;
            }
            waitFor(() -> slot.hasContent().get(), CHANGE_WAIT, made -> {
                if (!made) {
                    duplicating.run();
                    return;
                }
                settle(() -> hexColour(rgb(slot.color())), colour -> {
                    lp.put("made", true);
                    lp.put("colour", colour);
                    lp.put("sameAsMark", mark != null && mark.took() && sameColour(rgb(slot.color()), mark.got()));
                    if (!sameTrack(t, id)) {
                        lp.put("trackMoved", true);
                        duplicating.run();
                        return;
                    }
                    slot.deleteObject();
                    waitFor(() -> !slot.hasContent().get(), CHANGE_WAIT, removed -> {
                        lp.put("removed", removed);
                        duplicating.run();
                    }, done);
                }, done);
            }, done);
        }, done), done);
    }

    /** Arrange's second check: Bitwig's Duplicate on the clip in the launcher (see check). */
    private void duplicateCheck(String route, int t, String id, int index, Track track, Action launcherFocus, SettableBooleanValue launcherShown,
                                Action duplicate, Map<String, Object> checks, Finish done) {
        if (duplicate == null) {
            done.ok(null);
            return;
        }
        ClipLauncherSlot slot = trackSlots[t][index];
        Map<String, Object> d = new LinkedHashMap<>();
        Map<String, Object> ran = new LinkedHashMap<>();
        d.put("selected", false);
        d.put("focus", 0);
        ran.put("focus", null);
        ran.put("duplicate", null);
        d.put("ran", ran);
        d.put("launcher", 0);
        d.put("tracks", 0);
        d.put("removed", false);
        checks.put("duplicate", d);
        inLauncher(route, t, slot, track, launcherFocus, launcherShown, d, ran, () -> done.ok(null), () -> {
            int count = tracks.itemCount().get();
            boolean[] had = new boolean[SCENES];
            for (int s = 0; s < SCENES; s++) had[s] = trackSlots[t][s].hasContent().get();
            int clips = launcherClips(t);
            runBy(route, "Duplicate", duplicate::invoke, duplicated -> {
                ran.put("duplicate", duplicated);
                if (!duplicated) {
                    done.ok(null);
                    return;
                }
                waitFor(() -> launcherClips(t) != clips || tracks.itemCount().get() != count, CHANGE_WAIT, changed -> {
                    settle(() -> Arrays.asList(launcherClips(t), tracks.itemCount().get()), settled -> {
                        d.put("launcher", launcherClips(t) - clips);
                        d.put("tracks", tracks.itemCount().get() - count);
                        int made = -1;
                        for (int s = 0; s < SCENES && made < 0; s++)
                            if (!had[s] && trackSlots[t][s].hasContent().get()) made = s;
                        if (made < 0) {
                            done.ok(null);
                            return;
                        }
                        if (!sameTrack(t, id)) {
                            d.put("trackMoved", true);
                            done.ok(null);
                            return;
                        }
                        ClipLauncherSlot copy = trackSlots[t][made];
                        copy.deleteObject();
                        waitFor(() -> !copy.hasContent().get(), CHANGE_WAIT, removed -> {
                            d.put("removed", removed);
                            done.ok(null);
                        }, done);
                    }, done);
                }, done);
            }, done);
        }, done);
    }

    /**
     * For arrange's checks: selects a launcher slot on the track and gives the clip launcher the focus, as arrange does
     * before it copies, then runs `then`. `c` gets whether the slot got `selected` and how many times the focus action ran
     * (`focus`), and `ran` whether the route ran it (`focus`). When Bitwig doesn't select the slot, or the route doesn't
     * run the focus action (or its second go, which leaves the launcher hidden), the check stops there and `skip` runs
     * instead.
     */
    private void inLauncher(String route, int t, ClipLauncherSlot slot, Track track, Action launcherFocus, SettableBooleanValue launcherShown,
                            Map<String, Object> c, Map<String, Object> ran, Runnable skip, Runnable then, Finish done) {
        cursorTrack.selectChannel(track);
        slot.select();
        waitFor(() -> trackSelected[t].get() && slot.isSelected().get(), CHANGE_WAIT, selected -> {
            c.put("selected", selected);
            if (!selected) {
                skip.run();
                return;
            }
            if (launcherFocus == null) then.run();
            else focusPanel(route, FOCUS_LAUNCHER, launcherFocus, launcherShown, "clip launcher", c, "focus", ran, times -> {
                if (Boolean.FALSE.equals(ran.get("focus"))) skip.run();
                else then.run();
            }, done);
        }, done);
    }

    /** Whether the track at place `t` in the listing is still the one Bitwig keeps under `id` ("" when it hadn't said: then it's taken to be). */
    private boolean sameTrack(int t, String id) {
        return id.isEmpty() || id.equals(idOf(t));
    }

    /** Answers arrange with what Bitwig shows now, after the paste or without one. */
    private void answer(int t, Map<String, Object> m, Shown shown, Finish finish) {
        shown.launcher.put("after", arranger.isClipLauncherVisible().get());
        shown.timeline.put("after", arranger.isTimelineVisible().get());
        m.put("after", arrangerClipState());
        m.put("pastedColor", arrangerClipColour());
        m.put("launcherClipsAfter", launcherClips(t));
        finish.ok(m);
    }

    /** The clip selected in Bitwig's arranger: whether there is one, and the name of the track it's on (null without one). */
    private Map<String, Object> arrangerClipState() {
        boolean exists = arrangerClip.exists().get();
        Map<String, Object> m = new LinkedHashMap<>();
        m.put("exists", exists);
        m.put("track", exists ? str(arrangerClipTrack.name().get()) : null);
        return m;
    }

    /** The colour of the clip selected in Bitwig's arranger, written #RRGGBB, or null without one. */
    private String arrangerClipColour() {
        return arrangerClip.exists().get() ? hexColour(rgb(arrangerClip.color())) : null;
    }

    /** Whether the clip selected in Bitwig's arranger is on the track called `name`. */
    private boolean clipOn(String name) {
        return arrangerClip.exists().get() && str(arrangerClipTrack.name().get()).equals(name);
    }

    /** How many of a track's launcher slots (those the link sees) have a clip. */
    private int launcherClips(int t) {
        int count = 0;
        for (ClipLauncherSlot slot : trackSlots[t])
            if (slot.hasContent().get()) count++;
        return count;
    }

    /** How many launcher clips each track the link sees has (in its first slots), for telling where a file or a paste went. */
    private List<Integer> everyLauncherClip() {
        List<Integer> counts = new ArrayList<>(MAX_TRACKS);
        for (int t = 0; t < MAX_TRACKS; t++) counts.add(trackItems[t].exists().get() ? launcherClips(t) : 0);
        return counts;
    }

    // ---- Files as tracks (an experiment, extension 3.8) ----

    /**
     * An experiment (extension 3.8): inserts a file (a MIDI file, say) after a track, as dropping it between two tracks
     * does, and answers with where Bitwig put it. Copy and paste never put a launcher clip on the arranger timeline (live
     * run 20261015T185248Z, by every route), while this is a direct call of the API, the kind Bitwig carries out. A MIDI
     * file dropped on the arranger lands on its timeline, so Bitwig may make a new track with the clip there. When `at`
     * is given (beats from the start), the play start moves there first, in case Bitwig puts the clip at the play start.
     *
     * The answer, once Bitwig stops changing (or after CHANGE_WAIT without a change): `added` (how many tracks Bitwig
     * added), `index`, `name` and `type` (the first new track's, null without one), `tracksBefore` and `tracksAfter`
     * (Bitwig's track count), `launcherClips` (the new track's clips in its first launcher slots: more than 0 means Bitwig
     * put the file in the launcher), `selected` (whether the new track is the selected one), `arranger` (the clip selected
     * in the arranger: `exists`, and its `track`'s name), `arrangerBefore` (the same before), `at` (the play start as the
     * file went in), and `onTrack` (the given track's launcher clips `before` and `after`, in case Bitwig put the clip there
     * instead). The API can't list the clips on the arranger timeline, so only a look at Bitwig says for sure.
     *
     * Extension 3.9: the track can be given by its ID (`trackId`), and the new tracks are told by their IDs when Bitwig says
     * them: those it didn't have before, wherever they went in (where the listing first differs otherwise). The answer adds
     * the first new track's `id`, and `newTracks`, each new track's `index`, `id`, `name`, `type`, whether it's a `group`
     * and its `launcherClips`. A project file (.dawproject, .bwproject) may come in as tracks, or open as a project of its
     * own, in front: then the change stops, saying Bitwig has that open.
     */
    private void fileTrack(Map<String, Object> request, Finish finish) {
        if (trackAfter == null) throw new LinkError("This Bitwig didn't let the link insert files between tracks. A selftest says why.");
        if (arranger == null) throw new LinkError("This Bitwig didn't let the link reach its arranger. A selftest says why.");
        int t = requestedTrack(request);
        String targetId = idOf(t);
        String path = given(field(request, "path"), "file");
        Object given = field(request, "at");
        if (!absent(given) && !(given instanceof Double && Double.isFinite((Double) given) && (Double) given >= 0))
            throw new LinkError("Say where the play start goes as beats from the start (0 or more), not " + Json.js(given) + ".");
        Runnable insert = () -> {
            int count = tracks.itemCount().get();
            List<String> before = trackList();
            List<Integer> clipsBefore = everyLauncherClip();
            Set<String> idsBefore = new HashSet<>(tracksById().keySet());
            Map<String, Object> arrangerBefore = arrangerClipState();
            double at = transport.playStartPosition().get();
            trackAfter[t].insertFile(path);
            waitFor(() -> tracks.itemCount().get() != count || !trackList().equals(before) || !everyLauncherClip().equals(clipsBefore),
                    CHANGE_WAIT, changed -> {
                // Bitwig reports a change bit by bit, so what it did is read once it stops changing.
                settle(() -> Arrays.asList(tracks.itemCount().get(), trackList(), everyLauncherClip(), arrangerClipState()), settled -> {
                    int after = tracks.itemCount().get();
                    List<Integer> made = new ArrayList<>();
                    if (!idsBefore.isEmpty()) {
                        for (int i = 0; i < MAX_TRACKS; i++)
                            if (trackItems[i].exists().get() && !idOf(i).isEmpty() && !idsBefore.contains(idOf(i))) made.add(i);
                    } else if (after > count) {
                        int first = firstDifference(before, trackList());
                        if (first >= 0 && trackItems[first].exists().get()) made.add(first);
                    }
                    int n = made.isEmpty() ? -1 : made.get(0);
                    boolean seen = n >= 0;
                    Map<String, Object> m = new LinkedHashMap<>();
                    m.put("added", Math.max(0, after - count));
                    m.put("index", seen ? n : null);
                    m.put("id", seen && !idOf(n).isEmpty() ? idOf(n) : null);
                    m.put("name", seen ? str(trackItems[n].name().get()) : null);
                    m.put("type", seen ? str(trackItems[n].trackType().get()) : null);
                    m.put("tracksBefore", count);
                    m.put("tracksAfter", after);
                    m.put("launcherClips", seen ? launcherClips(n) : null);
                    m.put("selected", seen && trackSelected[n].get());
                    m.put("arranger", arrangerClipState());
                    m.put("arrangerBefore", arrangerBefore);
                    m.put("at", at);
                    // The track given may have moved down, when Bitwig put the new one before it: found by its ID when it has one.
                    int now = !targetId.isEmpty() ? indexOf(targetId) : seen && n <= t ? t + Math.max(0, after - count) : t;
                    Map<String, Object> on = new LinkedHashMap<>();
                    on.put("before", clipsBefore.get(t));
                    on.put("after", now >= 0 && now < MAX_TRACKS && trackItems[now].exists().get() ? launcherClips(now) : null);
                    m.put("onTrack", on);
                    List<Object> list = new ArrayList<>();
                    for (int i : made) {
                        Map<String, Object> track = new LinkedHashMap<>();
                        track.put("index", i);
                        track.put("id", idOf(i).isEmpty() ? null : idOf(i));
                        track.put("name", str(trackItems[i].name().get()));
                        track.put("type", str(trackItems[i].trackType().get()));
                        track.put("group", trackItems[i].isGroup().get());
                        track.put("launcherClips", launcherClips(i));
                        list.add(track);
                    }
                    m.put("newTracks", list);
                    finish.ok(m);
                }, finish);
            }, finish);
        };
        if (absent(given)) {
            insert.run();
            return;
        }
        double at = (Double) given;
        transport.playStartPosition().set(at);
        waitFor(() -> Math.abs(transport.playStartPosition().get() - at) < 1e-6, CHANGE_WAIT, moved -> {
            if (!moved) throw new LinkError("Bitwig didn't move the play start to beat " + Json.number(at) + ".");
            insert.run();
        }, finish);
    }

    // ---- Recording into the arranger (extension 3.9) ----

    /**
     * Records a launcher clip into Bitwig's arranger (extension 3.9), the way a person does: with Bitwig's arranger record on,
     * launching a clip while Bitwig is stopped starts it playing from the play start, and Bitwig records what the clip plays
     * onto its track's arranger timeline. Copy and paste never did this for the link (Bitwig copies whatever was clicked last
     * in it, not what the API selected), while every step here is an ordinary call of the API. Only the live tests send it
     * so far, in their own project.
     *
     * The request names the track by its ID (`trackId`), which stays put when a track goes in above it, the launcher `slot`,
     * where to record (`at`, beats from the start) and for how long (`length`, in beats), both whole bars. With `record`
     * false it plays the clip there without recording, to see whether the link can hear the track's notes at all. `arm`
     * arms the track while it records (the others are always disarmed, so they record nothing), `tempo` sets another tempo
     * meanwhile (the clip plays sooner at a faster one), and `deleteLauncherClip` deletes the launcher clip once it's
     * recorded, so the replay below can only hear the arranger's copy.
     *
     * Step by step:
     * <ol>
     * <li>It refuses while Bitwig plays, while arranger record is on, or while a launcher clip plays or is about to.</li>
     * <li>It keeps the transport's settings it changes (see recordSettings), the master track's mute and each track's arm,
     * then turns the metronome, the loop, punch in and out, overdub and automation write off, sets automation write to
     * touch (so nothing is written that isn't touched), no pre-roll and a launch quantize of a bar, mutes the master (so
     * nothing is heard), disarms the tracks, moves the play start to `at` and sets the tempo if asked. It reads each back,
     * and when one didn't take, puts everything back and answers `refused` with them.</li>
     * <li>It turns arranger record on (unless `record` is false) and launches the clip. While it plays, the link reads where
     * Bitwig is, the notes the track plays, and whether the slot plays and records. In the clip's last bar it stops the
     * track, which Bitwig does on the next bar line, at the clip's end.</li>
     * <li>Then, or just past the clip's end when the track still plays (`cut`: Bitwig once ignored the stop on a track that
     * wasn't armed), or once the time it should take has passed (`timedOut`), or recordabort asks, or someone stops Bitwig, it
     * turns record off, stops Bitwig (only if it's playing or may be about to: Stop while stopped moves the play start to the beginning) and
     * returns the track to the arrangement, so it plays its arranger timeline again.</li>
     * <li>After a recording, it deletes the launcher clip if asked, returns the track to the arrangement again, then plays the arranger back from `replayFrom` (`at`
     * unless given) to a bar past the clip's end, still muted, and notes the notes the track plays: with the launcher clip
     * not playing, those come from the arranger.</li>
     * <li>It puts back everything it changed, reads it back, and answers.</li>
     * </ol>
     * Nothing touches a parameter while record is on (Bitwig can write automation then), and every step checks that Bitwig
     * still has the project the job started in: when another comes to the front, the transport would steer that one, so the
     * job stops touching anything and answers `abandoned`. If the extension stops meanwhile, record goes off and Bitwig
     * stops (see emergency).
     *
     * The answer's `decided` sums it up: `recorded` (the replay heard the clip's notes with the launcher clip not playing),
     * `nothing` (they were heard as it played, not in the replay), `cantTell` (no notes heard either time, so only a look at
     * Bitwig says), `notChecked` (no replay); for a pass with record off, `heard` or `cantHear`; or `refused`, `didntStart`,
     * `aborted`, `abandoned` or `failed`. The rest says what the link saw (see Recording.answer).
     */
    private void record(Map<String, Object> request, Finish finish) {
        if (master == null) throw new LinkError("This Bitwig didn't let the link record into its arranger. A selftest says why.");
        if (!trackIds) throw new LinkError("This Bitwig doesn't tell the link tracks' IDs, which the record job finds its track by. A selftest says why.");
        Object id = field(request, "trackId");
        if (!(id instanceof String) || ((String) id).isEmpty()) throw new LinkError("Say which track to record by its ID (trackId).");
        Object s = field(request, "slot");
        if (!isIndex(s, SCENES))
            throw new LinkError("The app can reach the first " + SCENES + " clip slots on a track, not slot " + Json.js(s) + ".");
        double at = beats(field(request, "at"), "at", false);
        double length = beats(field(request, "length"), "length", true);
        boolean rec = flag(request, "record", true);
        boolean arm = flag(request, "arm", false);
        boolean delete = flag(request, "deleteLauncherClip", false);
        boolean replay = flag(request, "replay", rec);
        Object from = field(request, "replayFrom");
        double replayFrom = absent(from) ? at : beats(from, "replayFrom", false);
        Object t = field(request, "tempo");
        if (!absent(t) && !(t instanceof Double && (Double) t >= MIN_TEMPO && (Double) t <= MAX_TEMPO))
            throw new LinkError("The tempo is from " + Json.number(MIN_TEMPO) + " to " + Json.number(MAX_TEMPO) + " BPM, not " + Json.js(t) + ".");
        Double tempo = absent(t) ? null : (Double) t;
        Object pass = field(request, "pass");
        double bar = barLength();
        if (!wholeBars(at, bar) || !wholeBars(length, bar) || !wholeBars(replayFrom, bar))
            throw new LinkError("at, length and replayFrom have to be whole bars: " + Json.number(bar) + " beats each in "
                                + str(transport.timeSignature().get()) + ".");
        if (replayFrom > at) throw new LinkError("The replay can't start (replayFrom) after where the clip is recorded (at).");
        int track = indexOf((String) id);
        if (track < 0) throw new LinkError("There's no track with ID " + id + " in the project listing.");
        ClipLauncherSlot slot = trackSlots[track][index(s)];
        if (!slot.hasContent().get()) throw new LinkError("Slot " + index(s) + " on that track has no clip to record.");
        if (transport.isPlaying().get()) throw new LinkError("Bitwig is playing. Stop it, then ask again.");
        if (transport.isArrangerRecordEnabled().get()) throw new LinkError("Bitwig's arranger record is on. Turn it off, then ask again.");
        String busy = busyTrack();
        if (busy != null) throw new LinkError("A launcher clip on \"" + busy + "\" is playing, recording or about to. Stop it, then ask again.");
        new Recording(finish, (String) id, index(s), at, length, bar, rec, arm, delete, replay, replayFrom, tempo,
                      pass instanceof String ? (String) pass : null).start();
    }

    /** Stops the record job running now, if any (extension 3.9): it ends as soon as it next looks, and puts everything back. */
    private void abortRecording(Ask ask, Map<String, Object> request) {
        boolean running = recording != null && !recording.done;
        if (running) recording.abortAsked = true;
        reply(ask, Map.of("running", running), null);
    }

    /** The transport's switches the record job turns off while it records, and puts back afterwards. */
    private List<SettableBooleanValue> switches() {
        return List.of(transport.isArrangerRecordEnabled(), transport.isMetronomeEnabled(), transport.isArrangerLoopEnabled(),
                       transport.isPunchInEnabled(), transport.isPunchOutEnabled(), transport.isArrangerOverdubEnabled(),
                       transport.isClipLauncherOverdubEnabled(), transport.isArrangerAutomationWriteEnabled(),
                       transport.isClipLauncherAutomationWriteEnabled());
    }

    /**
     * What the record job sets before the clip plays, in the order it sets them and puts them back: the play start last, so
     * nothing set after it can move it.
     */
    private List<Setting> recordSettings(Double tempo, double at) {
        List<Setting> list = new ArrayList<>();
        list.add(onOff("metronome", transport.isMetronomeEnabled(), false));
        list.add(onOff("arranger loop", transport.isArrangerLoopEnabled(), false));
        list.add(onOff("punch in", transport.isPunchInEnabled(), false));
        list.add(onOff("punch out", transport.isPunchOutEnabled(), false));
        list.add(onOff("arranger overdub", transport.isArrangerOverdubEnabled(), false));
        list.add(onOff("launcher overdub", transport.isClipLauncherOverdubEnabled(), false));
        list.add(onOff("arranger automation write", transport.isArrangerAutomationWriteEnabled(), false));
        list.add(onOff("launcher automation write", transport.isClipLauncherAutomationWriteEnabled(), false));
        list.add(choice("automation write mode", transport.automationWriteMode(), "touch"));
        list.add(choice("pre-roll", transport.preRoll(), "none"));
        list.add(choice("launch quantize", transport.defaultLaunchQuantization(), "1"));
        list.add(onOff("master mute", master.mute(), true));
        if (tempo != null) list.add(new Setting("tempo", () -> transport.tempo().getRaw(), v -> transport.tempo().setRaw((Double) v), tempo));
        list.add(new Setting("play start", () -> transport.playStartPosition().get(), v -> transport.playStartPosition().set((Double) v), at));
        return list;
    }

    /** The record settings that keep Bitwig quiet: the job puts them back only once Bitwig has stopped. */
    private static final Set<String> QUIET = Set.of("master mute", "metronome");

    private static Setting onOff(String name, SettableBooleanValue value, boolean wanted) {
        return new Setting(name, () -> value.get(), v -> value.set((Boolean) v), wanted);
    }

    private static Setting choice(String name, SettableEnumValue value, String wanted) {
        return new Setting(name, () -> str(value.get()), v -> value.set((String) v), wanted);
    }

    /** One of Bitwig's settings the record job changes and puts back: its name (for the answer), how to read and set it, and what recording wants. */
    private record Setting(String name, Supplier<Object> read, Consumer<Object> write, Object wanted) {}

    /** Whether two settings' values are the same: numbers give or take a thousandth (Bitwig keeps a tempo to a few places). */
    static boolean same(Object a, Object b) {
        if (a instanceof Number && b instanceof Number) return Math.abs(((Number) a).doubleValue() - ((Number) b).doubleValue()) < 1e-3;
        return Objects.equals(a, b);
    }

    /** A bar's length in beats, from Bitwig's time signature ("4/4", "7/8"). */
    private double barLength() {
        Matcher m = Pattern.compile("^\\s*(\\d+)\\s*/\\s*(\\d+)").matcher(str(transport.timeSignature().get()));
        if (!m.find() || Integer.parseInt(m.group(1)) == 0 || Integer.parseInt(m.group(2)) == 0)
            throw new LinkError("The link can't tell a bar's length from Bitwig's time signature \"" + str(transport.timeSignature().get()) + "\".");
        return Integer.parseInt(m.group(1)) * 4.0 / Integer.parseInt(m.group(2));
    }

    private static boolean wholeBars(double beats, double bar) {
        double bars = beats / bar;
        return Math.abs(bars - Math.rint(bars)) < 1e-6;
    }

    /** A number of beats a request gives as `what`: 0 or more, or more than 0 when it's a length. */
    private static double beats(Object value, String what, boolean length) {
        if (!(value instanceof Double) || !Double.isFinite((Double) value) || (Double) value < 0 || (length && (Double) value == 0))
            throw new LinkError(what + " is a number of beats" + (length ? ", more than 0" : ", 0 or more") + ", not " + Json.js(value) + ".");
        return (Double) value;
    }

    /** A request's true-or-false field, or `otherwise` when it doesn't give one. */
    private static boolean flag(Map<String, Object> request, String key, boolean otherwise) {
        Object value = field(request, key);
        if (absent(value)) return otherwise;
        if (!(value instanceof Boolean)) throw new LinkError(key + " is true or false, not " + Json.js(value) + ".");
        return (Boolean) value;
    }

    /** The name of a track (one the link sees) with a launcher clip playing, recording or about to play, else null. */
    private String busyTrack() {
        for (int t = 0; t < MAX_TRACKS; t++) {
            if (!trackItems[t].exists().get()) continue;
            for (ClipLauncherSlot slot : trackSlots[t])
                if (slot.isPlaying().get() || slot.isPlaybackQueued().get() || slot.isRecording().get()) return str(trackItems[t].name().get());
        }
        return null;
    }

    /** The tracks the link sees, by ID, with their names (those Bitwig hasn't said the ID of are left out). */
    private Map<String, String> tracksById() {
        Map<String, String> m = new LinkedHashMap<>();
        for (int t = 0; t < MAX_TRACKS; t++) {
            String id = idOf(t);
            if (trackItems[t].exists().get() && !id.isEmpty()) m.put(id, str(trackItems[t].name().get()));
        }
        return m;
    }

    /** Where each track the link sees is in the listing now, by its ID. */
    private Map<String, Integer> placesById() {
        Map<String, Integer> m = new HashMap<>();
        for (int t = 0; t < MAX_TRACKS; t++) {
            String id = idOf(t);
            if (trackItems[t].exists().get() && !id.isEmpty()) m.putIfAbsent(id, t);
        }
        return m;
    }

    /** Whether each launcher slot the link sees has a clip, as "<track ID> <slot>". */
    private Map<String, Boolean> clipsById() {
        Map<String, Boolean> m = new LinkedHashMap<>();
        for (int t = 0; t < MAX_TRACKS; t++) {
            String id = idOf(t);
            if (!trackItems[t].exists().get() || id.isEmpty()) continue;
            for (int s = 0; s < SCENES; s++) m.put(id + " " + s, trackSlots[t][s].hasContent().get());
        }
        return m;
    }

    /**
     * One time the record job plays the clip (or the arranger back): where from, when it asked and when Bitwig started, and the
     * notes it heard the track play, each with where. Where Bitwig is comes from its play position, or, when that doesn't move
     * once Bitwig plays, from the clock and the tempo (`fromClock`). Until it first moves, it may still say where Bitwig last
     * stopped, so the clock says where Bitwig is meanwhile.
     */
    private static final class Playing {
        final double from;
        final long asked;
        Long started;
        double startPosition;
        boolean fromClock;
        boolean moved;
        int heard;
        final List<Object> notes = new ArrayList<>();
        private Set<Integer> held = new HashSet<>();
        String ended;

        Playing(double from, long asked) {
            this.from = from;
            this.asked = asked;
        }

        double position(long now, double reported, double tempo) {
            if (!moved && Math.abs(reported - startPosition) >= 1e-9) moved = true;
            if (!fromClock && !moved && now - started >= 500) fromClock = true;
            // The third live run: Bitwig still said 24.235, where the go before stopped, as it started a clip at 16, so the job
            // stopped the track as the clip launched.
            return fromClock || !moved ? from + (now - started) * tempo / 60000 : reported;
        }

        /** The track's notes now: each that wasn't playing a moment ago starts at `at`. */
        void hear(Object[] playing, double at) {
            Set<Integer> now = new HashSet<>();
            for (Object note : playing)
                if (note instanceof PlayingNote) now.add(((PlayingNote) note).pitch());
            for (Integer key : now) {
                if (held.contains(key)) continue;
                heard++;
                if (notes.size() >= MAX_NOTES) continue;
                Map<String, Object> note = new LinkedHashMap<>();
                note.put("key", key);
                note.put("at", Math.round(at * 1000) / 1000.0);
                notes.add(note);
            }
            held = now;
        }

        /** How many notes it heard from `start` (inclusive) to `end` (not). */
        int heardFrom(double start, double end) {
            int n = 0;
            for (Object note : notes) {
                double at = ((Number) ((Map<?, ?>) note).get("at")).doubleValue();
                if (at >= start - 0.01 && at < end) n++;
            }
            return n;
        }

        Map<String, Object> answer() {
            Map<String, Object> m = new LinkedHashMap<>();
            m.put("from", from);
            m.put("started", started != null);
            m.put("startedAfterMs", started == null ? null : started - asked);
            m.put("ended", ended);
            m.put("positionFrom", fromClock ? "clock" : "bitwig");
            m.put("heard", heard);
            m.put("notes", notes);
            return m;
        }
    }

    /** The record job running (see record): everything it keeps, sees and puts back, step by step. */
    private final class Recording {
        final Job job = currentJob;
        /** The project the job started in: it touches Bitwig only while that one is in front. */
        final String projectName = str(app.projectName().get());
        final Finish finish;
        final String trackId;
        final int scene;
        final double at;
        final double length;
        final double bar;
        final boolean rec;
        final boolean arm;
        final boolean delete;
        final boolean replay;
        final double replayFrom;
        final Double tempo;
        final String pass;
        final long began = clock.millis();
        final String trackName;
        List<Setting> settings;
        final Map<String, Object> saved = new LinkedHashMap<>();
        /** Each track's arm before, by its ID. */
        final Map<String, Boolean> arms = new LinkedHashMap<>();
        Map<String, String> tracksBefore;
        Map<String, Boolean> clipsBefore;
        double projectTempo;
        double tempoUsed;
        /** Whether the job has answered (or given up), so nothing it scheduled does anything more. */
        boolean done;
        boolean abortAsked;
        String abortWhy;
        /** Whether the job left out a step because it may not go on (see mayGoOn): it then answers `aborted`. */
        boolean cut;
        /** Whether the job asked for arranger record, and hasn't turned it off since. */
        boolean recordOn;
        /** When the job last asked Bitwig to start (a launch, or Play) without seeing it play since, else -1 (see stopPlaying). */
        long startAsked = -1;
        /** When the job last pressed Stop, and until when a stopped Bitwig doesn't count as stopped for sure (see stopped). */
        long stopSent = Long.MIN_VALUE / 2;
        long quietUntil = Long.MIN_VALUE / 2;
        /** Whether the job may have changed Bitwig's settings, so they need putting back. */
        boolean changed;
        final List<String> refused = new ArrayList<>();
        List<String> restoreFailed;
        Playing play;
        Playing replayed;
        long deadline;
        boolean slotPlayed;
        boolean slotRecording;
        boolean automationWriteSeenOn;
        Double stopSentAt;
        /** When the job first saw Bitwig past the clip's end, in clock time, else null. */
        Long endPassed;
        /**
         * What the job saw at each look and each command it sent, for working out what Bitwig did, a line each: ms since the
         * job started, what, Bitwig's play position ("@16.042"), then "playing", "slot" (the slot playing), "queued" (the
         * slot queued to play) and "notes 2" (notes the track plays) when they are so ("1234 look @16.042 playing slot notes 1").
         */
        final List<String> trace = new ArrayList<>();
        Boolean launcherClipDeleted;
        boolean replayLauncherPlayed;
        String error;

        Recording(Finish finish, String trackId, int scene, double at, double length, double bar, boolean rec, boolean arm, boolean delete,
                  boolean replay, double replayFrom, Double tempo, String pass) {
            this.finish = finish;
            this.trackId = trackId;
            this.scene = scene;
            this.at = at;
            this.length = length;
            this.bar = bar;
            this.rec = rec;
            this.arm = arm;
            this.delete = delete;
            this.replay = replay;
            this.replayFrom = replayFrom;
            this.tempo = tempo;
            this.pass = pass;
            trackName = str(trackItems[indexOf(trackId)].name().get());
        }

        /** Keeps what the job changes, sets it for recording, and waits for Bitwig to show it all. */
        void start() {
            recording = this;
            try {
                prepare();
            } catch (RuntimeException | LinkageError e) {
                error = failure(e);
                emergency();
                answer("failed");
            }
        }

        private void prepare() {
            settings = recordSettings(tempo, at);
            for (Setting s : settings) saved.put(s.name(), s.read().get());
            projectTempo = transport.tempo().getRaw();
            tempoUsed = tempo != null ? tempo : projectTempo;
            for (int t = 0; t < MAX_TRACKS; t++) {
                String id = idOf(t);
                if (trackItems[t].exists().get() && !id.isEmpty()) arms.put(id, trackItems[t].arm().get());
            }
            tracksBefore = tracksById();
            clipsBefore = clipsById();
            changed = true;
            for (Setting s : settings)
                if (!same(s.read().get(), s.wanted())) s.write().accept(s.wanted());
            Map<String, Integer> places = placesById();
            for (String id : arms.keySet()) {
                Integer t = places.get(id);
                boolean wanted = arm && id.equals(trackId);
                if (t != null && trackItems[t].arm().get() != wanted) trackItems[t].arm().set(wanted);
            }
            waitUntil(() -> notSet().isEmpty(), CHANGE_WAIT, set -> {
                refused.addAll(notSet());
                if (!refused.isEmpty() || !mayGoOn()) {
                    restore();
                    return;
                }
                if (!rec) {
                    launch();
                    return;
                }
                transport.isArrangerRecordEnabled().set(true);
                recordOn = true;
                trace("recordOn");
                waitUntil(() -> transport.isArrangerRecordEnabled().get(), CHANGE_WAIT, on -> {
                    if (!on) refused.add("arranger record");
                    if (!on) end("refused");
                    else launch();
                });
            });
        }

        /** The settings (and arms) that don't show what recording wants yet. */
        List<String> notSet() {
            List<String> list = new ArrayList<>();
            for (Setting s : settings)
                if (!same(s.read().get(), s.wanted())) list.add(s.name());
            boolean armsOk = true;
            Map<String, Integer> places = placesById();
            for (String id : arms.keySet()) {
                Integer t = places.get(id);
                if (t != null && trackItems[t].arm().get() != (arm && id.equals(trackId))) armsOk = false;
            }
            // A track past the ones the link sees may be armed too: Bitwig says whether any is.
            if (!armsOk || Link.this.project.hasArmedTracks().get() != arm) list.add("arm");
            return list;
        }

        /** Launches the clip, and watches it play. */
        void launch() {
            if (!mayGoOn()) {
                end("aborted");
                return;
            }
            long now = clock.millis();
            play = new Playing(at, now);
            deadline = now + START_WAIT + ms(length + 2 * bar);
            trackSlots[indexOf(trackId)][scene].launch();
            startAsked = now;
            trace("launch");
            later(this::watch);
        }

        /**
         * Whether the job may take its next step: it hasn't been asked to stop, "Let the app make changes" is still on, and
         * its track is still there (else it says why). Checked before each step after a wait, as for every change; putting
         * Bitwig back goes on regardless.
         */
        boolean mayGoOn() {
            if (!abortAsked && changesSetting.get() && target() >= 0) return true;
            if (!abortAsked && !changesSetting.get()) abortWhy = "Changes were turned off.";
            cut = true;
            return false;
        }

        /** One look at Bitwig while the clip plays (see record). */
        void watch() {
            if (!mayGoOn()) {
                end("aborted");
                return;
            }
            int t = indexOf(trackId);
            trace("look");
            long now = clock.millis();
            boolean playing = transport.isPlaying().get();
            if (play.started == null) {
                if (playing) {
                    play.started = now;
                    play.startPosition = transport.playPosition().get();
                    startAsked = -1;
                } else if (now - play.asked > START_WAIT) {
                    end("didntStart");
                    return;
                }
            } else if (!playing) {
                end("stoppedEarly");
                return;
            }
            ClipLauncherSlot slot = trackSlots[t][scene];
            slotPlayed |= slot.isPlaying().get();
            slotRecording |= slot.isRecording().get();
            automationWriteSeenOn |= transport.isArrangerAutomationWriteEnabled().get() || transport.isClipLauncherAutomationWriteEnabled().get();
            if (play.started != null) {
                double position = play.position(now, transport.playPosition().get(), tempoUsed);
                play.hear(trackItems[t].playingNotes().get(), position);
                if (stopSentAt == null && position >= at + length - bar - 1e-9) {
                    // On the next bar line, the clip's end, as the launch quantize is a bar.
                    trackItems[t].stop();
                    stopSentAt = position;
                    trace("stopTrack");
                }
                if (stopSentAt != null && position >= at + length - 1e-9) {
                    if (!slot.isPlaying().get() && !slot.isPlaybackQueued().get()) {
                        end("done");
                        return;
                    }
                    // Bitwig hasn't stopped the track at the clip's end (it once didn't on a track that wasn't armed): the job
                    // ends now, record off first, so what goes in the arranger ends here, not when the time runs out.
                    if (endPassed == null) endPassed = now;
                    else if (now - endPassed >= LATE_STOP) {
                        end("cut");
                        return;
                    }
                }
            }
            if (now >= deadline) {
                end("timedOut");
                return;
            }
            later(this::watch);
        }

        /** Ends the clip's playing (see record): record off, Bitwig stopped if it plays (or may be about to), the track back to the arrangement. */
        void end(String why) {
            if (play != null) play.ended = why;
            trace("end " + why);
            if (recordOn) transport.isArrangerRecordEnabled().set(false);
            stopPlaying();
            int t = indexOf(trackId);
            if (t >= 0) trackItems[t].returnToArrangement();
            waitUntil(() -> stopped() && !(recordOn && transport.isArrangerRecordEnabled().get()), CHANGE_WAIT, stopped -> {
                if (recordOn && !transport.isArrangerRecordEnabled().get()) recordOn = false;
                boolean recorded = rec && play != null && play.started != null
                                   && (why.equals("done") || why.equals("cut") || why.equals("timedOut") || why.equals("stoppedEarly"));
                if (!stopped || !recorded || !mayGoOn()) {
                    restore();
                    return;
                }
                if (delete) deleteClip(this::replayOrRestore);
                else replayOrRestore();
            });
        }

        /**
         * Presses Stop when Bitwig plays, or may be about to: a launch or Play the job asked for and hasn't seen start yet
         * (Bitwig takes commands in order, so the Stop still stops it). Never while Bitwig is stopped for sure, as Stop then
         * moves the play start back to 1.1.1. Bitwig counts as stopped for sure (see stopped) only once it has shown stopped
         * for SETTLE, and, after a start that hadn't shown, for as long as the job gives one to show (START_WAIT).
         */
        void stopPlaying() {
            boolean unconfirmed = startAsked >= 0;
            if (!unconfirmed && !transport.isPlaying().get()) return;
            long now = clock.millis();
            transport.stop();
            stopSent = now;
            quietUntil = Math.max(quietUntil, unconfirmed ? Math.max(now + SETTLE, startAsked + START_WAIT) : now + SETTLE);
            startAsked = -1;
        }

        /** Whether Bitwig has stopped for sure (see stopPlaying). While it still shows playing, presses Stop again every RESTOP. */
        boolean stopped() {
            long now = clock.millis();
            if (transport.isPlaying().get()) {
                quietUntil = Math.max(quietUntil, now + SETTLE);
                if (now - stopSent >= RESTOP) {
                    transport.stop();
                    stopSent = now;
                }
                return false;
            }
            return now >= quietUntil;
        }

        /** Deletes the launcher clip it recorded, so the replay can only hear the arranger's copy. */
        void deleteClip(Runnable then) {
            int t = indexOf(trackId);
            if (t < 0) {
                launcherClipDeleted = false;
                then.run();
                return;
            }
            trackSlots[t][scene].deleteObject();
            waitUntil(() -> {
                int now = indexOf(trackId);
                return now >= 0 && !trackSlots[now][scene].hasContent().get();
            }, CHANGE_WAIT, gone -> {
                launcherClipDeleted = gone;
                then.run();
            });
        }

        void replayOrRestore() {
            if (!replay || !mayGoOn()) {
                restore();
                return;
            }
            transport.playStartPosition().set(replayFrom);
            waitUntil(() -> same(transport.playStartPosition().get(), replayFrom), CHANGE_WAIT, moved -> {
                if (!moved || !mayGoOn()) {
                    restore();
                    return;
                }
                long now = clock.millis();
                replayed = new Playing(replayFrom, now);
                deadline = now + START_WAIT + ms(at + length + bar - replayFrom);
                // Again, as Bitwig once didn't take it after a recording: a track still on its launcher doesn't play the arranger.
                trackItems[indexOf(trackId)].returnToArrangement();
                transport.play();
                trace("replay");
                startAsked = now;
                later(this::watchReplay);
            });
        }

        /** One look at Bitwig while it plays the arranger back (see record). */
        void watchReplay() {
            if (!mayGoOn()) {
                stopReplay("aborted");
                return;
            }
            int t = indexOf(trackId);
            trace("replayLook");
            long now = clock.millis();
            boolean playing = transport.isPlaying().get();
            if (replayed.started == null) {
                if (playing) {
                    replayed.started = now;
                    replayed.startPosition = transport.playPosition().get();
                    startAsked = -1;
                } else if (now - replayed.asked > START_WAIT) {
                    stopReplay("didntStart");
                    return;
                }
            } else if (!playing) {
                stopReplay("stoppedEarly");
                return;
            }
            replayLauncherPlayed |= trackSlots[t][scene].isPlaying().get();
            if (replayed.started != null) {
                double position = replayed.position(now, transport.playPosition().get(), tempoUsed);
                replayed.hear(trackItems[t].playingNotes().get(), position);
                if (position >= at + length + bar - 1e-9) {
                    stopReplay("done");
                    return;
                }
            }
            if (now >= deadline) {
                stopReplay("timedOut");
                return;
            }
            later(this::watchReplay);
        }

        void stopReplay(String why) {
            replayed.ended = why;
            stopPlaying();
            waitUntil(this::stopped, CHANGE_WAIT, stopped -> restore());
        }

        /**
         * Puts back everything the job changed, once Bitwig has stopped for sure, then answers once Bitwig shows it all (or
         * after CHANGE_WAIT, saying what didn't come back).
         */
        void restore() {
            stopPlaying();
            waitUntil(this::stopped, CHANGE_WAIT, stopped -> {
                putBack(stopped);
                waitUntil(() -> notRestored().isEmpty(), CHANGE_WAIT, back -> {
                    restoreFailed = notRestored();
                    answer(null);
                });
            });
        }

        /**
         * Sets back each setting, arm and switch the job changed that doesn't show what it was; while Bitwig may still play
         * (not `stopped`), the master stays muted and the metronome off, so it makes no sound.
         */
        void putBack(boolean stopped) {
            if (recordOn && transport.isArrangerRecordEnabled().get()) transport.isArrangerRecordEnabled().set(false);
            for (Setting s : settings)
                if ((stopped || !QUIET.contains(s.name())) && !same(s.read().get(), saved.get(s.name()))) s.write().accept(saved.get(s.name()));
            Map<String, Integer> places = placesById();
            for (Map.Entry<String, Boolean> e : arms.entrySet()) {
                Integer t = places.get(e.getKey());
                if (t != null && trackItems[t].arm().get() != e.getValue()) trackItems[t].arm().set(e.getValue());
            }
        }

        List<String> notRestored() {
            List<String> list = new ArrayList<>();
            if (transport.isArrangerRecordEnabled().get()) list.add("arranger record");
            for (Setting s : settings)
                if (!same(s.read().get(), saved.get(s.name()))) list.add(s.name());
            Map<String, Integer> places = placesById();
            for (Map.Entry<String, Boolean> e : arms.entrySet()) {
                Integer t = places.get(e.getKey());
                if (t != null && trackItems[t].arm().get() != e.getValue()) {
                    list.add("arm");
                    break;
                }
            }
            return list;
        }

        /** The track's place in the listing now, found by its ID, or -1 when it has gone (the job then stops: see its callers). */
        int target() {
            int t = indexOf(trackId);
            if (t < 0) abortWhy = "The track went away.";
            return t;
        }

        /** Runs a step a moment from now, while the job may still go on (see still). */
        void later(Runnable step) {
            host.scheduleTask(() -> guarded(step), RECORD_TICK);
        }

        /** Waits until `test` is true, or `ms` have passed, then hands `then` whether it came true (see later). */
        void waitUntil(BooleanSupplier test, long ms, Then then) {
            long start = clock.millis();
            Runnable check = new Runnable() {
                @Override
                public void run() {
                    boolean ok = test.getAsBoolean();
                    if (ok || clock.millis() - start >= ms) then.run(ok);
                    else later(this);
                }
            };
            later(check);
        }

        /**
         * Runs a step of the job unless it has answered, or Bitwig has another project in front (then it answers `abandoned`
         * without touching anything more). A step that fails puts everything back as far as it can and answers `failed`.
         */
        void guarded(Runnable step) {
            if (done || job != currentJob) return;
            if (!projectName.equals(frontProject())) {
                answer("abandoned");
                return;
            }
            try {
                step.run();
            } catch (RuntimeException | LinkageError e) {
                if (done) return;
                error = failure(e);
                emergency();
                answer("failed");
            }
        }

        /**
         * Stops a recording at once, as far as it can, with nothing waited for: record off, Bitwig stopped if it plays (or may be about to), and
         * the settings put back. Only while Bitwig has the job's project in front: another's transport isn't the job's to
         * touch. For when Bitwig stops the link, and when a step fails (which then answers).
         */
        void emergency() {
            if (recording == this) recording = null;
            if (!projectName.equals(frontProject())) return;
            attemptQuietly(() -> {
                if (recordOn) transport.isArrangerRecordEnabled().set(false);
            });
            attemptQuietly(() -> {
                // A start asked for that hasn't shown yet still comes before this Stop.
                if (transport.isPlaying().get() || startAsked >= 0) transport.stop();
            });
            attemptQuietly(() -> {
                int t = indexOf(trackId);
                if (t >= 0) trackItems[t].returnToArrangement();
            });
            // The Stop goes first, so the master can be unmuted.
            if (changed) attemptQuietly(() -> putBack(true));
        }

        private void attemptQuietly(Runnable step) {
            try {
                step.run();
            } catch (RuntimeException | LinkageError e) {
                host.errorln("Music Organiser: couldn't put Bitwig back after recording (" + e + ").");
            }
        }

        long ms(double beats) {
            return (long) Math.ceil(beats * 60000 / tempoUsed);
        }

        /** Adds a line to the trace (see trace), while there's room. */
        void trace(String what) {
            if (trace.size() >= MAX_TRACE) return;
            int t = indexOf(trackId);
            int notes = t >= 0 ? trackItems[t].playingNotes().get().length : 0;
            trace.add((clock.millis() - began) + " " + what + String.format(Locale.ROOT, " @%.3f", transport.playPosition().get())
                      + (transport.isPlaying().get() ? " playing" : "") + (t >= 0 && trackSlots[t][scene].isPlaying().get() ? " slot" : "")
                      + (t >= 0 && trackSlots[t][scene].isPlaybackQueued().get() ? " queued" : "") + (notes > 0 ? " notes " + notes : ""));
        }

        /** What the job decided, from what it saw (see record), unless it already knows (`given`). */
        String decide(String given) {
            if (given != null) return given;
            if (!refused.isEmpty()) return "refused";
            if (error != null) return "failed";
            if (cut) return "aborted";
            if (play == null) return "refused";
            if (play.started == null) return "didntStart";
            if (!rec) return play.heard > 0 ? "heard" : "cantHear";
            if (replayed == null || replayed.started == null) return "notChecked";
            if (replayed.heardFrom(at, at + length + bar) > 0 && !replayLauncherPlayed) return "recorded";
            return play.heard > 0 ? "nothing" : "cantTell";
        }

        /** Answers with what the job saw and did: `given` is what it decided when it knows already (abandoned, failed). */
        void answer(String given) {
            if (done) return;
            done = true;
            if (recording == this) recording = null;
            String decided = decide(given);
            boolean front = projectName.equals(frontProject());
            Map<String, Object> m = new LinkedHashMap<>();
            if (pass != null) m.put("pass", pass);
            m.put("decided", decided);
            m.put("trackId", trackId);
            m.put("track", trackName);
            m.put("slot", scene);
            m.put("at", at);
            m.put("length", length);
            m.put("bar", bar);
            m.put("record", rec);
            m.put("arm", arm);
            Map<String, Object> tempos = new LinkedHashMap<>();
            tempos.put("project", projectTempo);
            tempos.put("used", tempoUsed);
            m.put("tempo", tempos);
            m.put("refused", refused);
            if (abortWhy != null) m.put("abortWhy", abortWhy);
            if (error != null) m.put("error", error);
            m.put("played", play == null ? null : play.answer());
            if (play != null) m.put("heard", play.heard);
            Map<String, Object> seen = new LinkedHashMap<>();
            seen.put("clipPlayed", slotPlayed);
            seen.put("slotRecording", slotRecording);
            seen.put("automationWriteSeenOn", automationWriteSeenOn);
            seen.put("stopSentAt", stopSentAt);
            m.put("seen", seen);
            m.put("launcherClipDeleted", launcherClipDeleted);
            Map<String, Object> back = null;
            if (replayed != null) {
                back = replayed.answer();
                back.put("to", at + length + bar);
                back.put("inPass", replayed.heardFrom(at, at + length + bar));
                back.put("before", replayed.heardFrom(Double.NEGATIVE_INFINITY, at));
                back.put("launcherPlayed", replayLauncherPlayed);
            }
            m.put("replay", back);
            // What Bitwig shows now, unless another project is in front: the job doesn't read that one's.
            Map<String, Object> after = new LinkedHashMap<>();
            after.put("checked", front);
            if (front) {
                after.put("recordOff", !transport.isArrangerRecordEnabled().get());
                after.put("playing", transport.isPlaying().get());
                after.put("playStart", transport.playStartPosition().get());
            }
            // Null when the job didn't wait to see everything come back (it was abandoned, or a step failed).
            after.put("restoreFailed", restoreFailed);
            m.put("after", after);
            m.put("diff", front && tracksBefore != null ? diff() : null);
            m.put("ms", clock.millis() - began);
            m.put("trace", trace);
            finish.ok(m);
        }

        /** The tracks added and removed since the job started (by ID), and the launcher slots whose clip came or went. */
        Map<String, Object> diff() {
            Map<String, String> tracksNow = tracksById();
            Map<String, Boolean> clipsNow = clipsById();
            List<Object> added = new ArrayList<>();
            for (Map.Entry<String, String> e : tracksNow.entrySet())
                if (!tracksBefore.containsKey(e.getKey())) {
                    Map<String, Object> track = new LinkedHashMap<>();
                    track.put("id", e.getKey());
                    track.put("name", e.getValue());
                    added.add(track);
                }
            List<Object> removed = new ArrayList<>();
            for (String id : tracksBefore.keySet())
                if (!tracksNow.containsKey(id)) removed.add(id);
            List<Object> clips = new ArrayList<>();
            for (Map.Entry<String, Boolean> e : clipsNow.entrySet()) {
                Boolean was = clipsBefore.get(e.getKey());
                if (was == null || was.equals(e.getValue())) continue;
                int cut = e.getKey().lastIndexOf(' ');
                Map<String, Object> c = new LinkedHashMap<>();
                c.put("trackId", e.getKey().substring(0, cut));
                c.put("slot", Integer.parseInt(e.getKey().substring(cut + 1)));
                c.put("before", was);
                c.put("after", e.getValue());
                clips.add(c);
            }
            Map<String, Object> m = new LinkedHashMap<>();
            m.put("tracksAdded", added);
            m.put("tracksRemoved", removed);
            m.put("clipsChanged", clips);
            return m;
        }
    }

    // ---- The probe (extension 3.9) ----

    /**
     * Probes whether Bitwig carries out the commands of its panels for the link (extension 3.9), by a route (`via`, see
     * ROUTES; DEFAULT_ROUTE without one): TOGGLE_ARRANGER, which shows or hides the arranger timeline whatever has the focus,
     * then FOCUS_LAUNCHER and FOCUS_ARRANGER, which bring their panel to the front, or hide it when it's in front already.
     * Each runs twice, `gap` ms apart (PROBE_GAP unless given), and the link reads whether its panel shows before and after
     * each go. A panel that shows and hides with them means Bitwig carried them out; one that never changes means it didn't
     * (as copy and paste weren't). It switches to the Arrange layout first, as arrange does, and shows or hides each panel
     * as it was afterwards. The answer: `via`, the `layout` before, and `steps`, each with its `action`, whether the route
     * `ran` each go, the panel's `visible` before and after each, and whether it was `restored` (null when it needn't be),
     * or `missing` when Bitwig has no such action.
     */
    private void probe(Map<String, Object> request, Finish finish) {
        String route = route(request);
        if (arranger == null) throw new LinkError("This Bitwig didn't let the link reach its arranger. A selftest says why.");
        Object g = field(request, "gap");
        if (!absent(g) && !(g instanceof Double && (Double) g >= 0 && (Double) g <= 10000))
            throw new LinkError("The gap is from 0 to 10000 ms, not " + Json.js(g) + ".");
        long gap = absent(g) ? PROBE_GAP : Math.round((Double) g);
        List<String> plan = List.of(TOGGLE_ARRANGER, FOCUS_LAUNCHER, FOCUS_ARRANGER);
        Map<String, Object> m = new LinkedHashMap<>();
        m.put("via", route);
        m.put("layout", str(app.panelLayout().get()));
        List<Object> steps = new ArrayList<>();
        m.put("steps", steps);
        Runnable go = () -> probeStep(route, plan, 0, gap, steps, () -> finish.ok(m), finish);
        if (arrangeLayout()) {
            go.run();
            return;
        }
        app.setPanelLayout(Application.PANEL_LAYOUT_ARRANGE);
        waitFor(this::arrangeLayout, CHANGE_WAIT, arranging -> {
            if (!arranging) throw new LinkError("Bitwig didn't switch to its Arrange layout.");
            go.run();
        }, finish);
    }

    private void probeStep(String route, List<String> plan, int i, long gap, List<Object> steps, Runnable then, Finish finish) {
        if (i >= plan.size()) {
            then.run();
            return;
        }
        String id = plan.get(i);
        Runnable next = () -> probeStep(route, plan, i + 1, gap, steps, then, finish);
        Map<String, Object> step = new LinkedHashMap<>();
        step.put("action", id);
        steps.add(step);
        Action action = app.getAction(id);
        if (action == null) {
            step.put("missing", true);
            next.run();
            return;
        }
        SettableBooleanValue visible = id.equals(FOCUS_LAUNCHER) ? arranger.isClipLauncherVisible() : arranger.isTimelineVisible();
        boolean before = visible.get();
        List<Object> ran = new ArrayList<>();
        List<Object> seen = new ArrayList<>(List.of(before));
        step.put("ran", ran);
        step.put("visible", seen);
        step.put("restored", null);
        runBy(route, id, action::invoke, first -> {
            ran.add(first);
            waitFor(() -> false, gap, w -> {
                seen.add(visible.get());
                runBy(route, id, action::invoke, second -> {
                    ran.add(second);
                    waitFor(() -> false, gap, w2 -> {
                        seen.add(visible.get());
                        if (visible.get() == before) {
                            next.run();
                            return;
                        }
                        visible.set(before);
                        waitFor(() -> visible.get() == before, CHANGE_WAIT, back -> {
                            step.put("restored", back);
                            next.run();
                        }, finish);
                    }, finish);
                }, finish);
            }, finish);
        }, finish);
    }

    // ---- Self-check ----

    /**
     * Sets up one part of Bitwig's API in init, timing it and keeping whatever goes wrong, so one part that
     * fails (in a Bitwig that changed) leaves the others working. `probe` reads it back for the self-check.
     */
    private void feature(String name, Runnable create, Supplier<String> probe) {
        long start = clock.millis();
        String error = null;
        try {
            create.run();
        } catch (RuntimeException | LinkageError e) {
            error = String.valueOf(e);
            host.errorln("Music Organiser: couldn't set up the " + name + " (" + e + ").");
        }
        features.add(new Feature(name, error, clock.millis() - start, probe));
    }

    /** Checks every part of Bitwig's API the link uses or offers, each on its own, and says how it went. */
    Map<String, Object> selfCheck(String why) {
        List<Object> checks = new ArrayList<>();
        check(checks, "API version", 0, () -> {
            int api = host.getHostApiVersion();
            if (api < MusicOrganiserExtensionDefinition.API_VERSION)
                throw new LinkError("Bitwig offers API " + api + "; the link needs " + MusicOrganiserExtensionDefinition.API_VERSION + ".");
            return "Bitwig offers API " + api + "; the link uses " + MusicOrganiserExtensionDefinition.API_VERSION + ".";
        });
        for (Feature f : features) {
            if (f.error() != null) checks.add(entry(f.name(), false, "Couldn't set it up: " + f.error(), f.ms()));
            else check(checks, f.name(), f.ms(), f.probe());
        }
        check(checks, "actions", 0, () -> app.getActions().length + " actions.");
        check(checks, "reports folder", 0, () -> {
            String why2 = reports.checkWritable();
            if (why2 != null) throw new LinkError(why2);
            String problem = reports.problem();
            return "Writing to " + reports.folder() + (problem == null ? "." : ". Earlier: " + problem);
        });
        boolean ok = true;
        for (Object c : checks) ok &= Boolean.TRUE.equals(((Map<?, ?>) c).get("ok"));

        Map<String, Object> m = new LinkedHashMap<>();
        m.put("time", reports.now());
        m.put("why", why);
        m.put("ok", ok);
        m.put("extension", MusicOrganiserExtensionDefinition.VERSION);
        if (!loadedFrom.isEmpty()) m.put("loadedFrom", loadedFrom);
        m.put("link", LINK_VERSION);
        m.put("product", safely(() -> str(host.getHostProduct())));
        m.put("bitwig", safely(() -> str(host.getHostVersion())));
        m.put("api", safely(host::getHostApiVersion));
        m.put("apiUsed", MusicOrganiserExtensionDefinition.API_VERSION);
        m.put("java", System.getProperty("java.version"));
        m.put("javaVendor", System.getProperty("java.vendor"));
        m.put("os", System.getProperty("os.name") + " " + System.getProperty("os.version"));
        m.put("platform", safely(() -> str(host.getPlatformType())));
        m.put("changes", safely(() -> changesSetting.get()));
        m.put("port", port);
        m.put("connections", clients.size());
        m.put("folder", reports.folder().toString());
        m.put("checks", checks);
        return m;
    }

    private void check(List<Object> checks, String name, long setupMs, Supplier<String> probe) {
        long start = clock.millis();
        try {
            String detail = probe.get();
            checks.add(entry(name, true, detail, setupMs + clock.millis() - start));
        } catch (RuntimeException | LinkageError e) {
            checks.add(entry(name, false, e instanceof LinkError ? e.getMessage() : String.valueOf(e), setupMs + clock.millis() - start));
        }
    }

    private static Map<String, Object> entry(String name, boolean ok, String detail, long ms) {
        Map<String, Object> m = new LinkedHashMap<>();
        m.put("name", name);
        m.put("ok", ok);
        m.put("detail", detail);
        m.put("ms", ms);
        return m;
    }

    /** A value for the self-check, or null when reading it fails (the checks say why). */
    private static Object safely(Supplier<Object> read) {
        try {
            return read.get();
        } catch (RuntimeException | LinkageError e) {
            return null;
        }
    }

    // ---- Helpers ----

    private int checkTrack(Object t) {
        if (!isIndex(t, MAX_TRACKS) || !trackItems[index(t)].exists().get())
            throw new LinkError("There's no track " + Json.js(t) + " in the project listing.");
        return index(t);
    }

    /**
     * The track a request names (extension 3.9): by the ID Bitwig keeps it under (`trackId`) when it gives one, which stays
     * put when a track goes in above it, else by its place in the project listing (`track`).
     */
    private int requestedTrack(Map<String, Object> request) {
        Object id = field(request, "trackId");
        if (absent(id)) return checkTrack(field(request, "track"));
        if (!(id instanceof String) || ((String) id).isEmpty()) throw new LinkError("A track's ID is text.");
        if (!trackIds) throw new LinkError("This Bitwig doesn't tell the link tracks' IDs, so it can't find one by its ID. A selftest says why.");
        int t = indexOf((String) id);
        if (t < 0) throw new LinkError("There's no track with ID " + id + " in the project listing.");
        return t;
    }

    /** The place in the listing of the track Bitwig keeps under `id`, or -1 when the link sees none (extension 3.9). */
    private int indexOf(String id) {
        if (!trackIds || id == null || id.isEmpty()) return -1;
        for (int t = 0; t < MAX_TRACKS; t++)
            if (trackItems[t].exists().get() && id.equals(str(trackItems[t].channelId().get()))) return t;
        return -1;
    }

    /** The ID Bitwig keeps the track at a place in the listing under, or "" when it hasn't said (or doesn't say IDs). */
    private String idOf(int t) {
        return trackIds ? str(trackItems[t].channelId().get()) : "";
    }

    private Device trackDevice(int t, Object d) {
        if (!isIndex(d, MAX_DEVICES) || !((Device) trackDevices[t].getItemAt(index(d))).exists().get())
            throw new LinkError("Track " + t + " has no device " + Json.js(d) + ".");
        return (Device) trackDevices[t].getItemAt(index(d));
    }

    /** Every track in the listing as "name|type" ("" where none), to tell which one is new. */
    private List<String> trackList() {
        List<String> list = new ArrayList<>(MAX_TRACKS);
        for (Track track : trackItems)
            list.add(track.exists().get() ? str(track.name().get()) + "|" + str(track.trackType().get()) : "");
        return list;
    }

    /**
     * Where the pad list starts, and what's on each pad (how many devices, less than 0 when the first is an empty
     * Sampler), to tell when it's caught up.
     */
    private List<Integer> padList() {
        List<Integer> list = new ArrayList<>(DRUM_PADS + 1);
        list.add(drumPads.scrollPosition().get());
        for (int p = 0; p < DRUM_PADS; p++)
            list.add(!padTaken(p) ? 0 : emptySampler(p) ? -padDevices[p].itemCount().get() : padDevices[p].itemCount().get());
        return list;
    }

    /** The first place two lists differ, or -1 if they don't. */
    static int firstDifference(List<?> a, List<?> b) {
        for (int i = 0; i < Math.max(a.size(), b.size()); i++) {
            Object x = i < a.size() ? a.get(i) : null;
            Object y = i < b.size() ? b.get(i) : null;
            if (x == null ? y != null : !x.equals(y)) return i;
        }
        return -1;
    }

    /** "#rrggbb" as red, green and blue from 0 to 1. */
    static float[] parseColour(Object text) {
        Matcher m = text instanceof String ? COLOUR.matcher((String) text) : null;
        if (m == null || !m.matches()) throw new LinkError("A colour is written #rrggbb.");
        return new float[] {
            Integer.parseInt(m.group(1), 16) / 255f, Integer.parseInt(m.group(2), 16) / 255f, Integer.parseInt(m.group(3), 16) / 255f
        };
    }

    private static UUID uuid(String text) {
        try {
            return UUID.fromString(text);
        } catch (IllegalArgumentException e) {
            throw new LinkError("A Bitwig device's ID is a UUID, such as " + SAMPLER + ", not " + text + ".");
        }
    }

    /** A VST2 plugin's ID: a number, written as Bitwig keeps it (a 32-bit whole number, as signed or not). */
    private static int vst2Id(Object plugin) {
        if (!(plugin instanceof Double)) throw new LinkError("A VST2 plugin's ID is a number.");
        double id = (Double) plugin;
        if (id != Math.rint(id) || id < Integer.MIN_VALUE || id > 0xFFFFFFFFL)
            throw new LinkError("A VST2 plugin's ID is a whole number that fits in 32 bits, not " + Json.js(plugin) + ".");
        return (int) (long) id;
    }

    static boolean isIndex(Object value, int count) {
        if (!(value instanceof Double)) return false;
        double v = (Double) value;
        return v >= 0 && v < count && Math.floor(v) == v;
    }

    /** An index that {@link #isIndex} has said is one. */
    private static int index(Object value) {
        return (int) (double) (Double) value;
    }

    private static String given(Object value, String what) {
        if (!(value instanceof String) || ((String) value).isEmpty()) throw new LinkError("No " + what + " given.");
        return (String) value;
    }

    /** A request's field, or {@link Json#UNDEFINED} if it doesn't have it. */
    private static Object field(Map<String, Object> map, String key) {
        return map.containsKey(key) ? map.get(key) : Json.UNDEFINED;
    }

    /** Whether a field is missing or null, which the link treats alike. */
    private static boolean absent(Object value) {
        return value == null || value == Json.UNDEFINED;
    }

    @SuppressWarnings("unchecked")
    private static Map<String, Object> asMap(Object value) {
        return value instanceof Map ? (Map<String, Object>) value : null;
    }

    static String str(Object value) {
        return value == null ? "" : String.valueOf(value);
    }

    private static String steps(int count) {
        return count < 0 ? "continuous" : count + " steps";
    }

    static String colour(SettableColorValue c) {
        return "#" + hex(c.red()) + hex(c.green()) + hex(c.blue());
    }

    private static String hex(float v) {
        int n = (int) Math.max(0, Math.min(255, Math.round(v * 255.0)));
        return (n < 16 ? "0" : "") + Integer.toHexString(n);
    }

    /** A colour as Bitwig reports it: red, green and blue, each from 0 to 1. */
    static float[] rgb(SettableColorValue c) {
        return new float[] {c.red(), c.green(), c.blue()};
    }

    /** A colour (red, green and blue from 0 to 1) written #RRGGBB, in capitals, as arrange's answer gives colours. */
    static String hexColour(float[] rgb) {
        return ("#" + hex(rgb[0]) + hex(rgb[1]) + hex(rgb[2])).toUpperCase(Locale.ROOT);
    }

    /** Whether two colours are the same, give or take SAME_COLOUR (out of 255) in each of red, green and blue. */
    static boolean sameColour(float[] a, float[] b) {
        for (int i = 0; i < 3; i++)
            if (Math.abs(a[i] - b[i]) * 255 > SAME_COLOUR + 0.01) return false;
        return true;
    }

    /** Whether two colours look clearly different: one of red, green and blue is OTHER_COLOUR (out of 255) or more apart. */
    static boolean clearlyDifferent(float[] a, float[] b) {
        for (int i = 0; i < 3; i++)
            if (Math.abs(a[i] - b[i]) * 255 >= OTHER_COLOUR - 0.01) return true;
        return false;
    }

    /**
     * A colour from its hue (0 to 1, round the colour wheel from red through green and blue), saturation and brightness (0
     * to 1 each), as red, green and blue in whole steps of 1/255, as a colour is written (#RRGGBB): so the colour asked
     * for is exactly the one the answer gives.
     */
    static float[] hsb(double hue, double saturation, double brightness) {
        double h = (hue - Math.floor(hue)) * 6;
        int sector = Math.min(5, (int) h);
        double f = h - sector;
        double v = brightness;
        double p = v * (1 - saturation);
        double q = v * (1 - saturation * f);
        double u = v * (1 - saturation * (1 - f));
        double[] c = switch (sector) {
            case 0 -> new double[] {v, u, p};
            case 1 -> new double[] {q, v, p};
            case 2 -> new double[] {p, v, u};
            case 3 -> new double[] {p, q, v};
            case 4 -> new double[] {u, p, v};
            default -> new double[] {v, p, q};
        };
        return new float[] {Math.round(c[0] * 255) / 255f, Math.round(c[1] * 255) / 255f, Math.round(c[2] * 255) / 255f};
    }

    // ---- Types ----

    /** A connection from the app. */
    static final class Client {
        final RemoteConnection connection;
        boolean open = true;

        Client(RemoteConnection connection) {
            this.connection = connection;
        }
    }

    /** A request being answered: who asked, its id, and what and when, for the log. */
    record Ask(Client client, Object id, String cmd, long started) {}

    /** Ends a change: with its result, or with an error message for the app. */
    interface Finish {
        void done(Object result, String error);

        default void ok(Object result) {
            done(result, null);
        }
    }

    /** What a wait found: whether what it waited for came true. */
    interface Then {
        void run(boolean ok);
    }

    interface Run {
        void run(Ask ask, Map<String, Object> request);
    }

    interface JobRun {
        void run(Map<String, Object> request, Finish finish);
    }

    record Command(boolean changes, Run run, JobRun job) {
        /** A command answered straight away. */
        static Command answer(boolean changes, Run run) {
            return new Command(changes, run, null);
        }

        /** A change that waits for Bitwig, so it needs changes turned on. */
        static Command job(JobRun job) {
            return new Command(true, null, job);
        }
    }

    /** A change that waits for Bitwig, and whether it needs changes turned on. */
    record Job(Ask ask, Map<String, Object> request, JobRun run, boolean changes) {}

    record Tuning(Parameter sampler, Parameter transpose) {}

    record Pad(int note, String path, String name) {}

    record Feature(String name, String error, long ms, Supplier<String> probe) {}

    /**
     * A cursor clip `notes` reads: where it's selected ("arranger" or "launcher"), the track it's on, whether that's the
     * selected track (null when Bitwig wouldn't say), and the notes starting in its grid, by channel, step and key.
     */
    static final class NoteClip {
        final String from;
        final Clip clip;
        final Track track;
        final BooleanValue selected;
        final Map<List<Integer>, GridNote> notes = new HashMap<>();

        NoteClip(String from, Clip clip, Track track, BooleanValue selected) {
            this.from = from;
            this.clip = clip;
            this.track = track;
            this.selected = selected;
        }
    }

    /** A note starting in a clip's grid: its step and key, its length in beats, and its velocity (0 to 1), as Bitwig said. */
    record GridNote(int step, int key, double length, double velocity) {}

    static final class Params {
        List<String> ids = List.of();
        final Map<String, String> names = new HashMap<>();
        final Map<String, Double> values = new HashMap<>();
        final Map<String, String> displays = new HashMap<>();
        /** How many times Bitwig has told each parameter's value. */
        final Map<String, Integer> told = new HashMap<>();
        /** What the last answer gave of each parameter (see `staleValue`). */
        final Map<String, Answered> answered = new HashMap<>();
    }

    record Answered(int told, String display, boolean stale) {}
}
