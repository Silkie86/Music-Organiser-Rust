package org.musicorganiser.bitwig;

import com.bitwig.extension.api.PlatformType;
import com.bitwig.extension.callback.ConnectionEstablishedCallback;
import com.bitwig.extension.callback.DataReceivedCallback;
import com.bitwig.extension.callback.DirectParameterDisplayedValueChangedCallback;
import com.bitwig.extension.callback.DirectParameterNameChangedCallback;
import com.bitwig.extension.callback.DirectParameterNormalizedValueChangedCallback;
import com.bitwig.extension.callback.NoArgsCallback;
import com.bitwig.extension.callback.StringArrayValueChangedCallback;
import com.bitwig.extension.callback.SysexMidiDataReceivedCallback;
import com.bitwig.extension.controller.api.Action;
import com.bitwig.extension.controller.api.ActionCategory;
import com.bitwig.extension.controller.api.Application;
import com.bitwig.extension.controller.api.Arranger;
import com.bitwig.extension.controller.api.BeatTimeValue;
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
import com.bitwig.extension.controller.api.DoubleValue;
import com.bitwig.extension.controller.api.DrumPad;
import com.bitwig.extension.controller.api.DrumPadBank;
import com.bitwig.extension.controller.api.EnumValue;
import com.bitwig.extension.controller.api.HardwareAction;
import com.bitwig.extension.controller.api.HardwareActionBindable;
import com.bitwig.extension.controller.api.HardwareActionMatcher;
import com.bitwig.extension.controller.api.HardwareBinding;
import com.bitwig.extension.controller.api.HardwareButton;
import com.bitwig.extension.controller.api.HardwareSurface;
import com.bitwig.extension.controller.api.InsertionPoint;
import com.bitwig.extension.controller.api.IntegerValue;
import com.bitwig.extension.controller.api.LastClickedParameter;
import com.bitwig.extension.controller.api.MasterRecorder;
import com.bitwig.extension.controller.api.MasterTrack;
import com.bitwig.extension.controller.api.MidiIn;
import com.bitwig.extension.controller.api.MidiOut;
import com.bitwig.extension.controller.api.NoteStep;
import com.bitwig.extension.controller.api.Parameter;
import com.bitwig.extension.controller.api.PinnableCursorDevice;
import com.bitwig.extension.controller.api.PlayingNote;
import com.bitwig.extension.controller.api.PlayingNoteArrayValue;
import com.bitwig.extension.controller.api.PopupBrowser;
import com.bitwig.extension.controller.api.Preferences;
import com.bitwig.extension.controller.api.Project;
import com.bitwig.extension.controller.api.RemoteConnection;
import com.bitwig.extension.controller.api.RemoteControl;
import com.bitwig.extension.controller.api.RemoteSocket;
import com.bitwig.extension.controller.api.SettableBeatTimeValue;
import com.bitwig.extension.controller.api.SettableBooleanValue;
import com.bitwig.extension.controller.api.SettableColorValue;
import com.bitwig.extension.controller.api.SettableEnumValue;
import com.bitwig.extension.controller.api.SettableIntegerValue;
import com.bitwig.extension.controller.api.SettableStringValue;
import com.bitwig.extension.controller.api.SoloValue;
import com.bitwig.extension.controller.api.SpecificBitwigDevice;
import com.bitwig.extension.controller.api.StringArrayValue;
import com.bitwig.extension.controller.api.StringValue;
import com.bitwig.extension.controller.api.TimeSignatureValue;
import com.bitwig.extension.controller.api.Track;
import com.bitwig.extension.controller.api.TrackBank;
import com.bitwig.extension.controller.api.Transport;
import java.lang.reflect.InvocationHandler;
import java.lang.reflect.InvocationTargetException;
import java.lang.reflect.Method;
import java.lang.reflect.Modifier;
import java.lang.reflect.Proxy;
import java.nio.charset.StandardCharsets;
import java.nio.file.Path;
import java.time.Clock;
import java.time.Instant;
import java.time.ZoneId;
import java.time.ZoneOffset;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.EnumSet;
import java.util.HashMap;
import java.util.HashSet;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.Optional;
import java.util.Set;
import java.util.UUID;
import java.util.concurrent.ConcurrentHashMap;
import java.util.function.BiConsumer;
import java.util.function.BooleanSupplier;
import java.util.function.Consumer;
import java.util.function.Supplier;
import java.util.regex.Pattern;

/**
 * A stand-in for the parts of Bitwig's controller API the extension uses, so it can be run and checked without
 * Bitwig: a port of the JavaScript script's stand-in (extensions/bitwig/test/fake-bitwig.js), made from Bitwig's
 * own interfaces with java.lang.reflect.Proxy. Like Bitwig, reading a value that init didn't mark as interested
 * fails, and changes the extension asks for happen a little later (on the stand-in's clock), reaching the extension
 * bit by bit, as Bitwig's do. Everything happens on the test's thread, as everything in Bitwig happens on its
 * controller thread.
 *
 * Each part of Bitwig here is a plain class whose methods are named as the API's are; {@link Stand} makes it the
 * API's interface. Calling anything a stand-in doesn't have fails and names it, so a test shows when the extension
 * starts to use more of Bitwig than this covers.
 *
 * Besides the project (tracks, tempo, actions...), the {@link Model} says what Bitwig would make of things the app
 * inserts (files, presets, plugins and knobs by name, a Drum Machine), as the JavaScript stand-in's model does,
 * and what's new to the extension: tracks' IDs, a device's remote controls page, the last clicked parameter, the
 * master recorder, the arranger (with copy and paste, for the arrange experiment, and launcher and arranger clips' colours,
 * which copy and paste carry with the clip), and parts of the API that fail, as in a Bitwig that changed.
 *
 * For the routes (extension 3.7), it has MIDI ports, which the model can loop back (what goes out comes back in a moment
 * later, as through a loopMIDI port set as both), and a hardware surface whose buttons a note coming back in presses. It
 * keeps track of where each call into the extension comes from (see {@link Context}), and the model says in which of
 * those Bitwig carries out the commands of its user interface: Bitwig 6.1.3 ignored them everywhere but, it seems, while
 * it handled a hardware button.
 *
 * For recording into the arranger (extension 3.9), it plays: the transport starts and stops (Stop while stopped moves the
 * play start to the beginning, as Bitwig's does), its play position runs on the stand-in's clock at the tempo, a launched
 * clip loops until its track stops on the next bar line, and with arranger record on, what a launcher clip plays goes in the
 * arranger on its track (see {@link Recorded}), which plays it back once the track is back on the arrangement. Each track
 * says the notes it's playing, from the notes the model gives each clip file. The transport's switches are kept, and the
 * model can make Bitwig ignore any of them, as well as stopping a track, or record nothing.
 */
final class FakeBitwig {
    static final String SAMPLER = Link.SAMPLER;
    static final String NOTE_TRANSPOSE = Link.NOTE_TRANSPOSE;
    private static final Pattern AUDIO = Pattern.compile("\\.(wav|aiff?|flac|mp3|ogg)$", Pattern.CASE_INSENSITIVE);
    private static final Pattern PRESET = Pattern.compile("\\.(bwpreset|vstpreset|fxb|fxp)$", Pattern.CASE_INSENSITIVE);
    private static final Pattern MIDI = Pattern.compile("\\.midi?$", Pattern.CASE_INSENSITIVE);
    /** The cursor device when it shows a device by itself (with show()), or none yet: sync leaves it alone. */
    private static final DeviceSpec ALONE = new DeviceSpec("(shown by itself)");

    final Model model;
    final FakeClock clock = new FakeClock();
    /** What the extension asked Bitwig to do: insertions, selections, actions run, a new track, the recorder. */
    final List<List<Object>> calls = new ArrayList<>();
    /** What the extension wrote to Bitwig's console: errors, and other lines. */
    final List<String> errors = new ArrayList<>();
    final List<String> printed = new ArrayList<>();
    /** The notifications it showed in Bitwig. */
    final List<String> popups = new ArrayList<>();
    /** What the extension sent on its MIDI out: ["sysex", hex as sent] and ["midi", status, data1, data2]. */
    final List<List<Object>> midi = new ArrayList<>();
    /** The user interface commands Bitwig ignored, as "<command> in <context>" (see Model.honoured). */
    final List<String> ignored = new ArrayList<>();
    /** Where the extension's code is running now: what called into it (see Context). */
    Context context = Context.TEST;
    /** The extension's buttons, on its hardware surface. */
    final List<Button> buttons = new ArrayList<>();
    private SysexMidiDataReceivedCallback sysexCallback;
    /** The size the extension gave its hardware surface, once it has. */
    double[] surfaceSize;
    final State state;
    final Link link;

    // The project, as Bitwig reports it.
    final Value projectName;
    final Value engine;
    final Value modified;
    final Param tempo;
    final Value signature;
    final Value playing = new Value(SettableBooleanValue.class, false);
    final Value position = new Value(SettableBeatTimeValue.class, 0.0);
    final Value browserOpen = new Value(BooleanValue.class, false);

    // Recording into the arranger (extension 3.9).
    /** The transport's switches and choices as Bitwig has them now, by the API's names (isMetronomeEnabled, preRoll...). */
    final Map<String, Object> transportNow = new LinkedHashMap<>();
    /** The same as the extension reads them. */
    private final Map<String, Value> transportValues = new LinkedHashMap<>();
    /** The tempo now, in BPM. */
    double tempoNow;
    /** Whether Bitwig plays, from where (in beats) and since when (the clock's time), and where it stopped. */
    boolean playingNow;
    private double playFrom;
    private long playSince;
    private double stoppedAt;
    /** Where Bitwig was when it last stopped playing, which its play position still says for a moment after it starts (see Model.positionLag). */
    private double lastStoppedAt;
    /** Where Bitwig was when arranger record went on as it played (null while it doesn't record). */
    private Double recordingFrom;
    /** Whether the master track is muted. */
    boolean masterMuted;
    final Value masterMute;
    final Value playPosition;
    private final Stand master;
    /** How many new tracks got an ID (Model.newTrackIds). */
    private int newIds;

    // The arranger, for the arrange experiment.
    /** The panels that can have the focus (copy and paste work on the one that has it), as {@link #focus} names them. */
    static final String LAUNCHER = "launcher";
    static final String TIMELINE = "timeline";
    /**
     * The panel layout Bitwig shows (Application.PANEL_LAYOUT_ARRANGE, _MIX or _EDIT), whether the arranger timeline and
     * the Arrange layout's clip launcher show, and the play start in beats.
     */
    final Value layout;
    final Value timeline;
    final Value launcher;
    final Value playStart;
    private String layoutNow;
    private boolean timelineShown;
    private boolean launcherShown;
    private double playStartAt;
    /** The panel with the focus: LAUNCHER, TIMELINE, or "" for another. */
    String focus;
    /** The launcher slot selected, as {track, scene}, or null. */
    private int[] slotSelected;
    /** What Bitwig's clipboard holds: the clip copied last, with its colour, or null. */
    Copied clipboard;
    /** The arranger's cursor clip: the clip selected in the arranger. */
    final ArrangerClip arrangerClip = new ArrangerClip();
    /** The cursor clips with a note grid (extension 3.15), the arranger's and the clip launcher's, once the extension made them. */
    GridClip arrangerGrid;
    GridClip launcherGrid;
    /** "Let the app make changes", once init has asked for it. */
    Value changes;

    final List<FakeTrack> trackItems = new ArrayList<>();
    Bank tracks;
    final FakeTrack cursorTrack;
    final CursorDevice cursorDevice;
    /** The pad list the cursor device made, once it has. */
    Pads pads;
    private int padScroll;
    final LastClicked lastClicked = new LastClicked();
    final Recorder recorder = new Recorder();
    /** The parameter last clicked in Bitwig, once one has been. */
    private Knob clicked;
    /** The clip slots each track in the bank has: the scenes the extension asked for. */
    private int bankScenes;

    /** What's selected: a track (its index, or -1), the device on it, and the device whose pads the pad list shows. */
    private int cursorIndex = -1;
    private DeviceSpec cursorSpec = ALONE;
    private DeviceSpec padsFrom;

    private final Map<String, DeviceSpec> builtIn = new HashMap<>();
    private final List<Stand> actions = new ArrayList<>();
    private ConnectionEstablishedCallback connect;
    final Stand host;

    /** Projects a test asked to bring to the front part way through a change (see switchDuring). */
    private final List<Switch> switches = new ArrayList<>();
    /** The change last seen waiting for Bitwig. */
    private Link.Job lastJob;

    private record Switch(String cmd, BooleanSupplier when, String project, Runnable change) {}

    /**
     * What called into the extension: Bitwig starting it (INIT), the app's connection (SOCKET), a task it scheduled
     * (TASK), Bitwig's flush (FLUSH), a SysEx message on its MIDI in (SYSEX), a press of one of its hardware buttons
     * (HARDWARE), or the test itself (TEST).
     */
    enum Context { INIT, SOCKET, TASK, FLUSH, SYSEX, HARDWARE, TEST }

    /** Runs `code` as called from `where` (see Context). */
    void in(Context where, Runnable code) {
        Context before = context;
        context = where;
        try {
            code.run();
        } finally {
            context = before;
        }
    }

    /**
     * Whether Bitwig carries out a command of its user interface (`what`: Save, copy, paste, a focus action) asked for
     * where the extension is running now: the model says where it does. One it ignores goes in `ignored`.
     */
    private boolean honoured(String what) {
        if (model.honoured.contains(context)) return true;
        ignored.add(what + " in " + context);
        return false;
    }

    /** The project as the stand-in keeps it: a test can change it, then refresh(). */
    static final class State {
        final List<TrackSpec> tracks = new ArrayList<>();
        int scenes;
        /** What Bitwig says the track count is while a new track is on its way, else null. */
        Integer count;
        /** The clips in the arranger, and the one selected there (null for none). */
        final List<Arranged> arranged = new ArrayList<>();
        Arranged arrangerSelected;
        /** The clips Bitwig recorded into the arranger from the launcher (extension 3.9). */
        final List<Recorded> recorded = new ArrayList<>();
        /**
         * The clips selected in the arranger and in the clip launcher, as their note grids show them (extension 3.15), or
         * null for none. The arranger's is apart from arrangerSelected, which only the arrange experiment's clip follows.
         */
        NoteClip arrangerNotes;
        NoteClip launcherNotes;
    }

    /**
     * A clip as a note grid shows it (extension 3.15): the name of its track, its loop start and length in beats, and its
     * notes. A test puts one in `state` (arrangerNotes or launcherNotes) to select it, then refresh().
     */
    static final class NoteClip {
        final String track;
        final double start;
        final double length;
        final List<ClipNote> notes = new ArrayList<>();

        NoteClip(String track, double start, double length) {
            this.track = track;
            this.start = start;
            this.length = length;
        }

        /** Adds a note on channel 1: where it starts and how long it is in beats, its key, and its velocity (0 to 1). */
        NoteClip note(double start, double length, int key, double velocity) {
            return note(0, start, length, key, velocity);
        }

        /** Adds a note on a channel (0 to 15). */
        NoteClip note(int channel, double start, double length, int key, double velocity) {
            notes.add(new ClipNote(channel, start, length, key, velocity));
            return this;
        }
    }

    /** A note in a clip: its channel (0 to 15), where it starts and how long it is (in beats), its key and velocity (0 to 1). */
    record ClipNote(int channel, double start, double length, int key, double velocity) {}

    /**
     * A launcher clip recorded into the arranger (extension 3.9): its track, where it starts and ends (in beats), the clip
     * it's of (the file it came from), and where that clip started playing, which its notes are counted from.
     */
    record Recorded(TrackSpec track, double start, double end, String clip, double clipStart) {}

    /** A launcher clip playing (or about to): its slot, where it starts (in beats) and where it stops (null while it loops on). */
    static final class Launched {
        final int scene;
        final double start;
        Double stopAt;

        Launched(int scene, double start) {
            this.scene = scene;
            this.start = start;
        }
    }

    /**
     * A clip in the arranger: its track, where it starts (in beats), the clip it's a copy of (the file it came from), and
     * its colour (#RRGGBB).
     */
    record Arranged(TrackSpec track, double start, String clip, String colour) {}

    /** What Bitwig's clipboard holds: a clip (the file it came from) and its colour (#RRGGBB), which a paste keeps. */
    record Copied(String clip, String colour) {}

    /**
     * The colour of a launcher clip that hasn't been given one, on a track without a colour of its own (a clip on a track
     * with one shows the track's, as a new clip in Bitwig does).
     */
    static final String CLIP_GREY = "#808080";

    private FakeBitwig(Model model, Path reports) {
        this.model = model;
        state = new State();
        for (TrackSpec t : model.tracks) state.tracks.add(t.copy());
        state.scenes = model.scenes;
        padScroll = model.padScroll;
        projectName = new Value(StringValue.class, model.project);
        engine = new Value(BooleanValue.class, model.engine);
        modified = new Value(BooleanValue.class, model.modified);
        tempoNow = model.tempo;
        tempo = new Param(Parameter.class, model.tempo, "") {
            /** Bitwig takes a tempo from 20 to 666 BPM, a moment later. */
            void setRaw(double value) {
                calls.add(Arrays.asList("tempo", value));
                later(20, () -> {
                    if (!model.settingsIgnored.contains("tempo")) tempoNow = Math.max(20, Math.min(666, value));
                });
            }
        };
        transportNow.putAll(model.transport);
        for (Map.Entry<String, Object> e : model.transport.entrySet()) {
            String name = e.getKey();
            Class<?> type = e.getValue() instanceof Boolean ? SettableBooleanValue.class : SettableEnumValue.class;
            Value value = new Value(type, e.getValue(), v -> setTransport(name, v));
            if (name.equals("isArrangerAutomationWriteEnabled"))
                value.live = () -> (Boolean) transportNow.get(name) || (model.automationWhileRecording && recordOn());
            transportValues.put(name, value);
        }
        masterMuted = model.masterMuted;
        masterMute = new Value(SettableBooleanValue.class, masterMuted, v -> {
            calls.add(Arrays.asList("masterMute", v));
            later(20, () -> {
                if (!model.settingsIgnored.contains("masterMute")) masterMuted = (Boolean) v;
            });
        });
        playPosition = new Value(BeatTimeValue.class, 0.0);
        playPosition.live = () -> model.positionStill && playingNow ? playFrom
                                  : playingNow && clock.now - playSince < model.positionLag ? lastStoppedAt : position();
        master = new Stand(MasterTrack.class) {
            Value mute() {
                return masterMute;
            }
        };
        signature = new Value(TimeSignatureValue.class, model.timeSignature);
        layoutNow = model.layout;
        timelineShown = model.timeline;
        launcherShown = model.launcher;
        playStartAt = model.playStart;
        focus = model.focus;
        clipboard = model.clipboard;
        layout = new Value(StringValue.class, layoutNow);
        timeline = new Value(SettableBooleanValue.class, timelineShown, v -> later(20, () -> timelineShown = (Boolean) v));
        launcher = new Value(SettableBooleanValue.class, launcherShown, v -> {
            calls.add(Arrays.asList("showLauncher", v));
            later(20, () -> {
                if (!model.launcherIgnored) showLauncher((Boolean) v);
            });
        });
        playStart = new Value(SettableBeatTimeValue.class, playStartAt, v -> {
            calls.add(Arrays.asList("playStart", v));
            later(20, () -> {
                if (!model.playStartIgnored) playStartAt = Math.max(0, ((Number) v).doubleValue());
            });
        });
        builtIn.put(SAMPLER, device("Sampler").id(SAMPLER).kind("note_to_audio").param("TRANSPOSE", 0));
        builtIn.put(NOTE_TRANSPOSE, device("Note Transpose").id(NOTE_TRANSPOSE).kind("note_to_note").param("COARSE", 0));
        if (model.drumMachine != null) {
            DeviceSpec machine = device("Drum Machine").id(model.drumMachine).kind("note_to_audio").drum();
            if (model.drumPads != null) {
                machine.pads = new LinkedHashMap<>();
                for (int note = 0; note < 128; note++) {
                    PadSpec pad = new PadSpec(model.drumPads.get(0).name);
                    for (DeviceSpec d : model.drumPads) pad.devices.add(d.copy());
                    machine.pads.put(note, pad);
                }
            }
            builtIn.put(model.drumMachine, machine);
        }
        for (ActionSpec a : model.actions) actions.add(action(a));

        cursorDevice = new CursorDevice();
        cursorTrack = new FakeTrack(-1);
        host = new Host();
        link = new Link((ControllerHost) host.api, new Reports(reports, clock), clock);
        link.loadedFrom = model.loadedFrom;
        link.markHue = model.markHue;
    }

    /**
     * Starts the extension in a stand-in Bitwig with the given project, writing its reports in `reports`, and
     * returns handles to poke at it.
     */
    static FakeBitwig start(Model model, Path reports) {
        FakeBitwig bitwig = new FakeBitwig(model, reports);
        bitwig.in(Context.INIT, bitwig.link::init);

        // Fill in the project after init, as Bitwig does.
        bitwig.sync();
        bitwig.cursorTrack.load(new TrackSpec(model.selectedTrack == null ? "" : model.selectedTrack, null));
        bitwig.cursorTrack.present.v = model.selectedTrack != null;
        for (int i = 0; i < bitwig.state.tracks.size(); i++) {
            if (bitwig.state.tracks.get(i).name.equals(model.selectedTrack)) {
                bitwig.cursorIndex = i;
                bitwig.sync();
                break;
            }
        }
        return bitwig;
    }

    /** Turns "Let the app make changes" on or off, as in Bitwig's settings. */
    void allowChanges(boolean on) {
        changes.set(on);
    }

    /** Tells the extension about changes a test made to `state` itself. */
    void refresh() {
        sync();
    }

    /** Clicks a parameter in Bitwig: the extension hears of it a moment later. */
    void click(Knob knob) {
        later(20, () -> clicked = knob);
    }

    /**
     * Brings another project to the front part way through the next change asked for with `cmd` ("newtrack",
     * "pads"...), as if someone did it in Bitwig meanwhile: once the extension, running that change, first waits for
     * Bitwig. Only the project's name changes: the tracks the stand-in shows from then on stand for that project's, as
     * Bitwig's track bank follows the project in front.
     */
    void switchDuring(String cmd, String project) {
        switches.add(new Switch(cmd, null, project, null));
    }

    /** As {@link #switchDuring(String, String)}, at the change's first wait after `when` comes true (say, once samples are dropped on pads). */
    void switchDuring(String cmd, BooleanSupplier when, String project) {
        switches.add(new Switch(cmd, when, project, null));
    }

    /** Changes something in Bitwig (`change`, on `state`) part way through the next `cmd`, at its first wait after `when` comes true. */
    void during(String cmd, BooleanSupplier when, Runnable change) {
        switches.add(new Switch(cmd, when, null, change));
    }

    /** The extension has just started to wait for Bitwig: switches the project if a test asked for it at this point of the change running. */
    private void waiting() {
        Link.Job job = link.runningJob();
        if (job == null) return;
        boolean first = job != lastJob;
        lastJob = job;
        for (Switch s : new ArrayList<>(switches)) {
            if (!s.cmd().equals(job.request().get("cmd")) || !(s.when() != null ? s.when().getAsBoolean() : first)) continue;
            switches.remove(s);
            if (s.project() != null) projectName.v = s.project();
            if (s.change() != null) later(0, s.change());
        }
    }

    /** The track the extension's bank shows at `index`. */
    FakeTrack track(int index) {
        return trackItems.get(index);
    }

    // ---- The world: what happens in Bitwig ----

    /** Makes a change a moment from now, then tells the extension, as Bitwig does. */
    void later(long ms, Runnable change) {
        clock.schedule(() -> {
            change.run();
            sync();
        }, ms);
    }

    private TrackSpec trackSpec(int index) {
        return index >= 0 && index < state.tracks.size() ? state.tracks.get(index) : null;
    }

    private TrackSpec cursorSpec() {
        return trackSpec(cursorIndex);
    }

    private void selectTrack(FakeTrack track) {
        later(20, () -> {
            TrackSpec spec = trackSpec(track.index);
            if (spec == null || cursorIndex == track.index) return;
            cursorIndex = track.index;
            cursorSpec = spec.devices.isEmpty() ? null : spec.devices.get(0);
        });
    }

    private void selectDevice(FakeDevice device) {
        FakeTrack owner = null;
        for (FakeTrack t : trackItems)
            if (t.devices != null && t.devices.items.contains(device)) owner = t;
        if (owner == null) return;
        int t = owner.index;
        int index = owner.devices.items.indexOf(device);
        later(20, () -> {
            TrackSpec track = trackSpec(t);
            DeviceSpec spec = track != null && index < track.devices.size() ? track.devices.get(index) : null;
            if (spec == null) return;
            cursorIndex = t;
            cursorSpec = spec;
        });
        // The pads follow a moment after the device.
        clock.schedule(() -> {
            padsFrom = cursorSpec;
            sync();
        }, 60);
    }

    private DeviceSpec padDevice() {
        DeviceSpec d = padsFrom;
        return d != null && d.drum ? d : null;
    }

    private PadSpec padAt(int index) {
        DeviceSpec d = padDevice();
        return d != null && d.pads != null ? d.pads.get(padScroll + index) : null;
    }

    /** Something inserted into track `t`'s chain: what Bitwig would make of it, if anything. */
    private void insert(int t, String at, String method, Object arg) {
        String key = String.valueOf(arg);
        DeviceSpec spec;
        switch (method) {
            case "insertFile":
                spec = model.files.get(key);
                if (spec == null && AUDIO.matcher(key).find()) spec = builtIn.get(SAMPLER).copy().preset(baseName(key));
                if (spec == null && model.presets && PRESET.matcher(key).find()) {
                    String name = baseName(key);
                    int cut = name.indexOf(" - ");
                    String plugin = cut < 0 ? name : name.substring(0, cut);
                    String preset = cut < 0 ? "" : name.substring(cut + 3);
                    spec = device(plugin).preset(preset.isEmpty() ? plugin : preset).plugin().kind("note_to_audio");
                }
                break;
            case "insertBitwigDevice":
                spec = builtIn.get(key);
                break;
            default:
                String format = method.equals("insertVST3Device") ? "VST3" : method.equals("insertVST2Device") ? "VST2" : "CLAP";
                spec = model.plugins.get(format + ":" + key);
        }
        if (spec == null) return;
        if (spec.knobs == null && model.knobs.containsKey(spec.name)) spec = spec.copy().knobs(model.knobs.get(spec.name));
        TrackSpec track = state.tracks.get(t);
        DeviceSpec made = spec.copy();
        later(40, () -> {
            if (at.equals("start")) track.devices.add(0, made);
            else track.devices.add(made);
        });
    }

    private void createInstrumentTrack(int position) {
        calls.add(Arrays.asList("createInstrumentTrack", position));
        // The count changes first; the track itself shows up a moment later, after the last main track.
        later(30, () -> state.count = state.tracks.size() + 1);
        later(60, () -> {
            state.count = null;
            int main = -1;
            for (int i = 0; i < state.tracks.size() && main < 0; i++) {
                String type = state.tracks.get(i).type;
                if ("Effect".equals(type) || "Master".equals(type)) main = i;
            }
            int at = main < 0 ? state.tracks.size() : main;
            int instruments = 0;
            for (TrackSpec t : state.tracks)
                if (t.type == null || t.type.isEmpty() || t.type.equals("Instrument")) instruments++;
            TrackSpec made = new TrackSpec("Inst " + (instruments + 1), "Instrument");
            if (model.newTrackIds) made.id = "new-" + (++newIds);
            state.tracks.add(at, made);
            if (cursorIndex >= 0 && cursorIndex >= at) cursorIndex++;
            if (model.browserOnNewTrack) browserOpen.v = true;
        });
    }

    /** Selects a launcher slot (unless the model says Bitwig ignores that), and gives the launcher the focus if the model says so. */
    private void selectSlot(int track, int scene) {
        calls.add(Arrays.asList("selectSlot", track, scene));
        later(20, () -> {
            if (model.selectIgnored) return;
            slotSelected = new int[] {track, scene};
            if (model.selectFocuses && launcherShown) focus = LAUNCHER;
        });
    }

    /** Shows or hides the clip launcher in the Arrange layout: hidden, it loses the focus. */
    private void showLauncher(boolean shown) {
        launcherShown = shown;
        if (!shown && LAUNCHER.equals(focus)) focus = "";
    }

    /** The API's copy, which Bitwig carries out only where the model says (see copied). */
    private void copy() {
        calls.add(List.of("copy"));
        if (honoured("copy")) copied();
    }

    /**
     * Copy, in the panel with the focus: Bitwig's clipboard takes the launcher clip selected (with the clip launcher in
     * front), or the clip selected in the arranger (with the arranger timeline in front), with its colour. With another
     * panel in front, or nothing selected, the clipboard keeps what it had.
     */
    private void copied() {
        if (model.copyIgnored) return;
        later(20, () -> {
            Copied copied = null;
            if (LAUNCHER.equals(focus) && launcherShown && slotSelected != null) {
                TrackSpec track = trackSpec(slotSelected[0]);
                int scene = slotSelected[1];
                if (track != null && scene < track.slots.size() && track.slots.get(scene) != null)
                    copied = new Copied(track.slots.get(scene), clipColour(track, scene));
            } else if (TIMELINE.equals(focus) && timelineShown && state.arrangerSelected != null) {
                copied = new Copied(state.arrangerSelected.clip(), state.arrangerSelected.colour());
            }
            if (copied != null) clipboard = copied;
        });
    }

    /** A launcher clip's colour: its own, or else its track's (CLIP_GREY on a track without one). */
    static String clipColour(TrackSpec track, int scene) {
        String own = track.clipColours.get(scene);
        if (own != null) return own;
        return track.colour != null ? hex(track.colour) : CLIP_GREY;
    }

    /** A colour (red, green and blue from 0 to 1) as the stand-in keeps it: #RRGGBB. */
    static String hex(float[] rgb) {
        return String.format(Locale.ROOT, "#%02X%02X%02X", Math.round(rgb[0] * 255), Math.round(rgb[1] * 255), Math.round(rgb[2] * 255));
    }

    /** The API's paste, which Bitwig carries out only where the model says (see pasted). */
    private void paste() {
        calls.add(List.of("paste"));
        if (honoured("paste")) pasted();
    }

    /**
     * Paste: with the arranger timeline shown and in front, a copy of the clip goes on the selected track at the play
     * start (or the track the model says), and is selected there (so the arranger's cursor clip follows it); with the clip launcher in front, it goes in
     * the slot selected when that's empty, else the track's next empty slot after it. Either way the copy has the colour the clip had when it was
     * copied. With another panel in front, nothing happens. The model can make Bitwig ignore paste, or paste into the
     * launcher whichever panel is in front.
     */
    private void pasted() {
        later(40, () -> {
            TrackSpec track = cursorSpec();
            if (model.pasteIgnored || clipboard == null || track == null) return;
            if (TIMELINE.equals(focus) && timelineShown && !model.pasteInLauncher) {
                TrackSpec on = model.arrangerPasteOn == null ? track
                        : state.tracks.stream().filter(t -> t.name.equals(model.arrangerPasteOn)).findFirst().orElseThrow();
                Arranged made = new Arranged(on, playStartAt, clipboard.clip(), clipboard.colour());
                state.arranged.add(made);
                state.arrangerSelected = made;
            } else if (LAUNCHER.equals(focus) && launcherShown || model.pasteInLauncher) {
                // Into the slot selected when it's empty, else the next empty one after it.
                int s = slotSelected != null && slotSelected[0] == cursorIndex ? slotSelected[1] : 0;
                while (s < track.slots.size() && track.slots.get(s) != null) s++;
                while (track.slots.size() <= s) track.slots.add(null);
                track.slots.set(s, clipboard.clip());
                track.clipColours.put(s, clipboard.colour());
                state.scenes = Math.max(state.scenes, s + 1);
            }
        });
    }

    /**
     * Duplicate, in the panel with the focus: with the clip launcher in front, a copy of the launcher clip selected goes in
     * the track's next empty slot after it, in the same colour, and is selected; with the arranger timeline in front, the
     * clip selected there gets a copy after it (where the copy starts doesn't matter here); with another panel in front,
     * the selected track gets a copy after it. The model can make Bitwig ignore Duplicate.
     */
    private void duplicated() {
        later(40, () -> {
            if (model.duplicateIgnored) return;
            if (LAUNCHER.equals(focus) && launcherShown && slotSelected != null) {
                TrackSpec track = trackSpec(slotSelected[0]);
                int scene = slotSelected[1];
                if (track == null || scene >= track.slots.size() || track.slots.get(scene) == null) return;
                int s = scene + 1;
                while (s < track.slots.size() && track.slots.get(s) != null) s++;
                while (track.slots.size() <= s) track.slots.add(null);
                track.slots.set(s, track.slots.get(scene));
                track.clipColours.put(s, clipColour(track, scene));
                state.scenes = Math.max(state.scenes, s + 1);
                slotSelected = new int[] {slotSelected[0], s};
            } else if (TIMELINE.equals(focus) && timelineShown && state.arrangerSelected != null) {
                Arranged a = state.arrangerSelected;
                Arranged copy = new Arranged(a.track(), a.start() + 4, a.clip(), a.colour());
                state.arranged.add(copy);
                state.arrangerSelected = copy;
            } else if (cursorSpec() != null) {
                TrackSpec copy = cursorSpec().copy();
                copy.name = copy.name + " 2";
                state.tracks.add(cursorIndex + 1, copy);
            }
        });
    }

    /**
     * A file inserted after track `t` (the filetrack experiment): a MIDI file goes on a new track after it, named after
     * the file, in the arranger at the play start or in the new track's first launcher slot, as the model says.
     */
    private void fileAfter(int t, String method, Object arg) {
        if (!method.equals("insertFile") || !MIDI.matcher((String) arg).find() || model.fileGoes.equals("nothing")) return;
        String path = (String) arg;
        later(40, () -> {
            TrackSpec made = new TrackSpec(baseName(path), "Instrument");
            if (model.fileGoes.equals("launcher")) {
                made.slots.add(path);
                state.scenes = Math.max(state.scenes, 1);
            }
            state.tracks.add(t + 1, made);
            if (cursorIndex > t) cursorIndex++;
            if (slotSelected != null && slotSelected[0] > t) slotSelected = new int[] {slotSelected[0] + 1, slotSelected[1]};
            if (model.fileGoes.equals("arranger")) {
                Arranged clip = new Arranged(made, playStartAt, path, CLIP_GREY);
                state.arranged.add(clip);
                if (model.fileSelected) state.arrangerSelected = clip;
            }
        });
    }

    /**
     * Bitwig's focus_or_toggle_arranger and focus_or_toggle_clip_launcher actions (`id`): each brings its panel to the
     * front, or hides it when it's in front already. The model can make Bitwig ignore one, or have it hide its panel but
     * never show it.
     */
    private void focusOrToggle(String id) {
        boolean arranger = id.equals(Link.FOCUS_ARRANGER);
        String panel = arranger ? TIMELINE : LAUNCHER;
        later(20, () -> {
            if (model.focusIgnored.contains(id)) return;
            boolean shown = arranger ? timelineShown : launcherShown;
            if (panel.equals(focus) && shown) {
                if (arranger) timelineShown = false;
                else launcherShown = false;
                focus = "";
            } else if (shown || !model.focusOnlyHides.contains(id)) {
                if (arranger) timelineShown = true;
                else launcherShown = true;
                focus = panel;
            }
        });
    }

    /** Bitwig's toggle_arranger action: shows the arranger timeline, or hides it (it then loses the focus). */
    private void toggleArranger() {
        later(20, () -> {
            timelineShown = !timelineShown;
            if (!timelineShown && TIMELINE.equals(focus)) focus = "";
        });
    }

    // ---- Playing and recording (extension 3.9) ----

    /** One of the transport's switches or choices set by the extension: it changes a moment later, unless the model says Bitwig ignores it. */
    private void setTransport(String name, Object value) {
        calls.add(Arrays.asList("transport", name, value));
        later(20, () -> {
            if (model.settingsIgnored.contains(name)) return;
            if (name.equals("isArrangerRecordEnabled")) record((Boolean) value);
            else transportNow.put(name, value);
        });
    }

    boolean recordOn() {
        return Boolean.TRUE.equals(transportNow.get("isArrangerRecordEnabled"));
    }

    /** Arranger record going on or off: while Bitwig plays, what's recorded so far goes in the arranger when it goes off. */
    private void record(boolean on) {
        if (on == recordOn()) return;
        if (on && playingNow) recordingFrom = position();
        if (!on) recorded(position());
        transportNow.put("isArrangerRecordEnabled", on);
    }

    /** Where Bitwig is, in beats: running on from where it started at the tempo while it plays. */
    double position() {
        return playingNow ? playFrom + (clock.now - playSince) * tempoNow / 60000 : stoppedAt;
    }

    /** A bar's length in beats, from the time signature ("4/4", "7/8"). */
    double bar() {
        String[] parts = ((String) signature.v).split("/");
        return Integer.parseInt(parts[0].trim()) * 4.0 / Integer.parseInt(parts[1].trim());
    }

    /** Launches and Plays still to start (see Model.startDelay), and how many Stops have cancelled those asked for before them. */
    private int startsDue;
    private int stops;

    private void startPlaying(double from) {
        playingNow = true;
        playFrom = from;
        playSince = clock.now;
        if (recordOn()) recordingFrom = from;
    }

    /** Bitwig stops: what was recorded goes in the arranger, launcher clips stop, and the playhead goes back to the play start. */
    private void stopPlaying() {
        recorded(position());
        lastStoppedAt = position();
        playingNow = false;
        stoppedAt = playStartAt;
        for (TrackSpec t : state.tracks) t.launched = null;
    }

    /**
     * Recording ends at `end` (beats): each launcher clip that played meanwhile goes in the arranger, on its track, for the
     * time it played, unless the model says Bitwig records nothing, or only on armed tracks.
     */
    private void recorded(double end) {
        Double from = recordingFrom;
        recordingFrom = null;
        if (from == null || !playingNow || model.recordsNothing) return;
        for (TrackSpec t : state.tracks) {
            Launched l = t.launched;
            if (l == null || (model.recordsOnlyArmed && !t.arm) || l.scene >= t.slots.size() || t.slots.get(l.scene) == null) continue;
            double start = Math.max(from, l.start);
            double stop = Math.min(end, l.stopAt == null ? end : l.stopAt);
            if (stop > start + 1e-9) state.recorded.add(new Recorded(t, start, stop, t.slots.get(l.scene), l.start));
        }
    }

    /** A launcher clip launched: with Bitwig stopped, it starts playing from the play start, and the clip with it. */
    private void launch(int track, int scene) {
        calls.add(Arrays.asList("launch", track, scene));
        int stops = this.stops;
        startsDue++;
        later(model.startDelay, () -> {
            startsDue--;
            if (stops != this.stops) return;
            TrackSpec t = trackSpec(track);
            if (t == null || scene >= t.slots.size() || t.slots.get(scene) == null) return;
            double start;
            if (playingNow) {
                start = nextBar(position());
            } else {
                startPlaying(playStartAt);
                start = playStartAt;
            }
            t.launched = new Launched(scene, start);
            t.launcherMode = true;
        });
    }

    /** The next bar line after `beats` (the one after it, when it's on one). */
    private double nextBar(double beats) {
        double bar = bar();
        return (Math.floor(beats / bar + 1e-9) + 1) * bar;
    }

    /** A track's launcher clip stopping: on the next bar line, as the launch quantize is a bar (or straight away without one). */
    private void stopTrack(int track) {
        calls.add(Arrays.asList("stopTrack", track));
        later(20, () -> {
            TrackSpec t = trackSpec(track);
            if (model.trackStopIgnored || t == null || t.launched == null || !playingNow) return;
            double now = position();
            String quantize = (String) transportNow.get("defaultLaunchQuantization");
            t.launched.stopAt = "none".equals(quantize) ? now : nextBar(now);
        });
    }

    /** A track back on the arrangement: its launcher clip stops, and it plays its arranger timeline again. */
    private void returnToArrangement(int track) {
        calls.add(Arrays.asList("returnToArrangement", track));
        later(20, () -> {
            TrackSpec t = trackSpec(track);
            if (t == null) return;
            t.launched = null;
            t.launcherMode = false;
        });
    }

    /** Whether a track's launcher clip in `scene` is playing now (null scene: any). */
    private boolean clipPlaying(TrackSpec t, Integer scene) {
        Launched l = t == null ? null : t.launched;
        if (l == null || !playingNow || (scene != null && l.scene != scene)) return false;
        double now = position();
        return now >= l.start - 1e-9 && (l.stopAt == null || now < l.stopAt - 1e-9);
    }

    private boolean clipQueued(TrackSpec t, int scene) {
        Launched l = t == null ? null : t.launched;
        return l != null && playingNow && l.scene == scene && position() < l.start - 1e-9;
    }

    /**
     * The notes a track is playing now: its launcher clip's while that plays, else (back on the arrangement) those of what
     * was recorded into its arranger timeline where Bitwig is. None while Bitwig is stopped, or when the model hides them.
     */
    private PlayingNote[] notesOf(TrackSpec t) {
        if (t == null || !playingNow || model.notesHidden) return new PlayingNote[0];
        double now = position();
        List<PlayingNote> list = new ArrayList<>();
        if (clipPlaying(t, null)) {
            notesAt(t.slots.get(t.launched.scene), now - t.launched.start, list);
        } else if (!t.launcherMode) {
            for (Recorded r : state.recorded)
                if (r.track() == t && now >= r.start() - 1e-9 && now < r.end() - 1e-9) notesAt(r.clip(), now - r.clipStart(), list);
        }
        return list.toArray(new PlayingNote[0]);
    }

    /** The notes of a clip file (Model.clipNotes) sounding `beats` into it, as it loops (Model.clipLength). */
    private void notesAt(String clip, double beats, List<PlayingNote> into) {
        List<double[]> notes = model.clipNotes.getOrDefault(clip, List.of());
        double length = model.clipLength.getOrDefault(clip, 4.0);
        double at = ((beats % length) + length) % length;
        for (double[] n : notes)
            if (at >= n[0] - 1e-9 && at < n[0] + n[1] - 1e-9) into.add(playingNote((int) n[2]));
    }

    private static PlayingNote playingNote(int key) {
        return (PlayingNote) new Stand(PlayingNote.class) {
            int pitch() {
                return key;
            }

            int velocity() {
                return 100;
            }
        }.api;
    }

    /** Tells the extension how things are now, as Bitwig's observers do. */
    private void sync() {
        for (Map.Entry<String, Value> e : transportValues.entrySet()) e.getValue().v = transportNow.get(e.getKey());
        tempo.v = tempoNow;
        playing.v = playingNow;
        masterMute.v = masterMuted;
        layout.v = layoutNow;
        timeline.v = timelineShown;
        launcher.v = launcherShown;
        playStart.v = playStartAt;
        Arranged selectedClip = state.arrangerSelected;
        arrangerClip.present.v = selectedClip != null;
        arrangerClip.colour.show(selectedClip == null ? null : selectedClip.colour());
        arrangerClip.track.present.v = selectedClip != null;
        arrangerClip.track.title.v = selectedClip == null ? "" : selectedClip.track().name;
        TrackSpec selectedTrack = cursorSpec();
        String cursorName = selectedTrack == null ? null : selectedTrack.name;
        if (arrangerGrid != null) arrangerGrid.show(state.arrangerNotes, cursorName);
        if (launcherGrid != null) launcherGrid.show(state.launcherNotes, cursorName);
        if (tracks == null) return;   // the extension couldn't make its track bank: nothing more to tell it
        tracks.count.v = state.count == null ? state.tracks.size() : state.count;
        for (int i = 0; i < trackItems.size(); i++) trackItems.get(i).load(trackSpec(i));
        TrackSpec selected = cursorSpec();
        if (selected != null) {
            cursorTrack.present.v = true;
            cursorTrack.title.v = selected.name;
        }
        DeviceSpec device = cursorSpec;
        if (device != ALONE) {
            cursorDevice.load(device);
            cursorDevice.where.v = device != null && selected != null ? selected.devices.indexOf(device) : -1;
            cursorDevice.drum.v = device != null && device.drum;
            cursorDevice.follow(device);
        }
        cursorDevice.showPage();
        if (pads != null) {
            DeviceSpec from = padDevice();
            pads.scroll.v = padScroll;
            for (int i = 0; i < pads.items.size(); i++) {
                Pad pad = (Pad) pads.items.get(i);
                PadSpec p = from != null && from.pads != null ? from.pads.get(padScroll + i) : null;
                pad.present.v = p != null;
                pad.title.v = p != null ? p.name : "";
                pad.chain.count.v = p != null ? p.devices.size() : 0;
                for (int d = 0; d < pad.chain.items.size(); d++)
                    ((FakeDevice) pad.chain.items.get(d)).load(p != null && d < p.devices.size() ? p.devices.get(d) : null);
            }
        }
        lastClicked.parameter.show(clicked);
        recorder.active.v = recorder.recording;
    }

    // ---- Talking to the extension as the app does ----

    /** Connects as the app would, and returns a way to send requests and read what comes back. */
    App connect() {
        return new App();
    }

    /** A connection from the app. Requests go in through the extension's receive callback; replies come out of send. */
    final class App {
        final List<Map<String, Object>> received = new ArrayList<>();
        private int read;
        private DataReceivedCallback receive;
        private NoArgsCallback disconnect;
        /** Whether the connection has broken without Bitwig noticing: sending on it fails. */
        private boolean broken;

        private App() {
            Stand connection = new Stand(RemoteConnection.class) {
                void setReceiveCallback(DataReceivedCallback callback) {
                    receive = callback;
                }

                void setDisconnectCallback(NoArgsCallback callback) {
                    disconnect = callback;
                }

                void send(byte[] bytes) {
                    if (broken) throw new IllegalStateException("The connection to the app is broken.");
                    received.add(message(bytes));
                }
            };
            in(Context.SOCKET, () -> connect.connectionEstablished((RemoteConnection) connection.api));
        }

        /** Messages that arrived since the last call. */
        List<Map<String, Object>> take() {
            List<Map<String, Object>> fresh = new ArrayList<>(received.subList(read, received.size()));
            read = received.size();
            return fresh;
        }

        /** Sends a request (a map, written as JSON, or text as it is) as Bitwig would hand it over: UTF-8 bytes. */
        void send(Object request) {
            String text = request instanceof String ? (String) request : Json.write(request);
            in(Context.SOCKET, () -> receive.dataReceived(text.getBytes(StandardCharsets.UTF_8)));
        }

        /** Sends a request and returns the one reply to it. */
        Map<String, Object> ask(Object request) {
            send(request);
            return only(take());
        }

        /** Sends a request, lets the stand-in's clock run (for changes that wait) and returns the reply. */
        Map<String, Object> waitFor(Object request) {
            return waitFor(request, 10000);
        }

        Map<String, Object> waitFor(Object request, long ms) {
            send(request);
            clock.advance(ms);
            return only(take());
        }

        void close() {
            in(Context.SOCKET, disconnect::call);
        }

        /** Breaks the connection without Bitwig noticing, so the extension's next send fails. */
        void breakConnection() {
            broken = true;
        }

        private Map<String, Object> only(List<Map<String, Object>> fresh) {
            if (fresh.size() != 1) throw new AssertionError("expected one reply, got " + Json.write(fresh));
            return fresh.get(0);
        }
    }

    /** A message from the extension: plain ASCII JSON ending in a newline, as the app reads it. */
    @SuppressWarnings("unchecked")
    private static Map<String, Object> message(byte[] bytes) {
        for (byte b : bytes)
            if (b < 0) throw new AssertionError("expected ASCII bytes, got " + (b & 0xff));
        String text = new String(bytes, StandardCharsets.US_ASCII);
        if (!text.endsWith("\n") || text.indexOf('\n') != text.length() - 1 || text.indexOf('\r') >= 0)
            throw new AssertionError("expected one line of JSON ending in a newline, got " + text);
        Object parsed = Json.parse(text);
        if (!(parsed instanceof Map)) throw new AssertionError("expected a JSON object, got " + text);
        return (Map<String, Object>) parsed;
    }

    // ---- What the project is made of ----

    /** What the stand-in Bitwig has open, and what it makes of things the app inserts. */
    static final class Model {
        /** The file Bitwig loaded the extension from, as the extension tells it ("" when it can't). */
        String loadedFrom = "";
        String project = "Untitled";
        double tempo = 120;
        String timeSignature = "4/4";
        boolean modified;
        /** Whether Bitwig's audio engine is on (the master recorder only records when it is). */
        boolean engine = true;
        /** The selected track's name, or null when none is selected. */
        String selectedTrack;
        final List<TrackSpec> tracks = new ArrayList<>();
        final List<ActionSpec> actions = new ArrayList<>();
        /** A file's path, and the device it loads (a preset). */
        final Map<String, DeviceSpec> files = new HashMap<>();
        /** Whether any preset file loads, as the plugin its name starts with ("Serum - Lead.vstpreset"). */
        boolean presets;
        /** "VST3:<id>", "VST2:<number>" or "CLAP:<id>", and the device that plugin is. */
        final Map<String, DeviceSpec> plugins = new HashMap<>();
        /** A device's name ("Serum"), and the knobs it has when the app inserts it. */
        final Map<String, List<Knob>> knobs = new HashMap<>();
        /** The UUID that adds a Drum Machine here (Bitwig's own isn't known). */
        String drumMachine;
        /** How many scenes the project has. */
        int scenes = 8;
        /** Where the pad list starts. */
        int padScroll;
        /** Whether a new track opens Bitwig's browser. */
        boolean browserOnNewTrack;
        /**
         * The devices on every pad of a new Drum Machine, as a saved default Drum Machine can have (Solomon's has an
         * empty Sampler on each), or null for none.
         */
        List<DeviceSpec> drumPads;
        /** The port Bitwig gives the link when 19791 is taken, or 0 when it isn't. */
        int portInUse;
        /** How many times Bitwig says -1 when asked which port it gave (Bitwig 6.1.3 does, straight after making the connection). */
        int portUnknown;
        /** The Bitwig version and API version the host reports (the script's stand-in reports the same). */
        String version = "6.1.2";
        int api = 25;
        /** Methods of Bitwig's host that fail here, as in a Bitwig that doesn't have them. */
        final Set<String> broken = new HashSet<>();

        // The arranger, for the arrange experiment.
        /** The panel layout Bitwig shows at first: Application.PANEL_LAYOUT_ARRANGE, _MIX or _EDIT. */
        String layout = Application.PANEL_LAYOUT_ARRANGE;
        /** Whether the arranger timeline, and the clip launcher in the Arrange layout, show at first. */
        boolean timeline = true;
        boolean launcher = true;
        /** Where the play start is at first, in beats. */
        double playStart;
        /** How long a launch, or Play, takes to start Bitwig playing, in ms (the record job looks every 25 ms). */
        long startDelay = 20;
        /** How long Bitwig takes to make a clip from a file put in a launcher slot, in ms (longer from a cold disk). */
        long clipDelay = 40;
        /** The panel with the focus at first: LAUNCHER, TIMELINE, or "" for another. */
        String focus = "";
        /**
         * Whether selecting a launcher slot gives the clip launcher the focus, as clicking one in Bitwig does. Off, as
         * selecting one from the API may not.
         */
        boolean selectFocuses;
        /** Whether Bitwig ignores selecting a launcher slot, switching the panel layout, showing the clip launcher or moving the play start. */
        boolean selectIgnored;
        boolean layoutIgnored;
        boolean launcherIgnored;
        boolean playStartIgnored;
        /** Bitwig's focus_or_toggle_ actions (by ID) it ignores, and those that hide their panel when it's in front but never show it. */
        final Set<String> focusIgnored = new HashSet<>();
        final Set<String> focusOnlyHides = new HashSet<>();
        /** Whether Bitwig ignores paste, and copy (its clipboard keeps what it held). */
        boolean pasteIgnored;
        boolean copyIgnored;
        /** Whether Bitwig ignores Duplicate (extension 3.8's checks run it). */
        boolean duplicateIgnored;
        /**
         * Where Bitwig puts a MIDI file inserted after a track (the filetrack experiment, extension 3.8): on a new track
         * after it, in the arranger at the play start ("arranger") or in the new track's first launcher slot ("launcher"),
         * or nowhere ("nothing"). The new track is named after the file.
         */
        String fileGoes = "arranger";
        /** Whether a clip Bitwig puts in the arranger that way is selected there, so the arranger's cursor clip follows it. */
        boolean fileSelected = true;
        /** Whether Bitwig pastes into the clip launcher, whichever panel has the focus. */
        boolean pasteInLauncher;
        /** The track Bitwig's paste in the arranger timeline goes on instead of the selected one, or null for that one. */
        String arrangerPasteOn;
        /** What's on Bitwig's clipboard at first, as if someone had copied a clip in their own song, or null for nothing. */
        Copied clipboard;
        /** Whether Bitwig ignores giving a launcher clip a colour. */
        boolean colourIgnored;
        /**
         * Whether Bitwig rounds a launcher clip's colour to one of its own, as it may to its palette: here each of red, green
         * and blue to the nearest multiple of 0x33.
         */
        boolean colourSnaps;
        /** The hue the link's marking colours start from (see Link.markHue), so a test knows which colours come. */
        double markHue;

        // The routes (extension 3.7).
        /**
         * Where Bitwig carries out the commands of its user interface (Save, its Copy and Paste actions, the API's copy and
         * paste, the focus actions): everywhere unless a test says otherwise. Elsewhere it ignores them, as Bitwig 6.1.3 did.
         */
        final Set<Context> honoured = EnumSet.allOf(Context.class);
        /** Whether the extension's MIDI out comes back on its MIDI in (a loopMIDI port set as both), and after how many milliseconds. */
        boolean loopback;
        long loopbackDelay = 5;
        /** Once this says true, the loopback brings nothing more back, as if its port went away part way (null: never). */
        BooleanSupplier loopbackCut;
        /** Whether Bitwig never calls the extension's flush, however often it asks. */
        boolean noFlush;

        // Recording into the arranger (extension 3.9).
        /**
         * The transport's switches and choices at first, by the API's names: a few on, and pre-roll, automation write and
         * the launch quantize not as the record job wants them, so a test sees them put back.
         */
        final Map<String, Object> transport = new LinkedHashMap<>();
        {
            transport.put("isArrangerRecordEnabled", false);
            transport.put("isMetronomeEnabled", true);
            transport.put("isArrangerLoopEnabled", true);
            transport.put("isPunchInEnabled", false);
            transport.put("isPunchOutEnabled", false);
            transport.put("isArrangerOverdubEnabled", true);
            transport.put("isClipLauncherOverdubEnabled", false);
            transport.put("isArrangerAutomationWriteEnabled", false);
            transport.put("isClipLauncherAutomationWriteEnabled", false);
            transport.put("automationWriteMode", "latch");
            transport.put("preRoll", "one_bar");
            transport.put("defaultLaunchQuantization", "1/4");
        }
        /** What Bitwig ignores when the extension sets it: the transport's (by the API's names), "tempo" or "masterMute". */
        final Set<String> settingsIgnored = new HashSet<>();
        /** Whether the master track is muted at first. */
        boolean masterMuted;
        /** The notes each clip file plays, each {start, length, key} in beats, and how long each clip is (it loops; 4 beats if not given). */
        final Map<String, List<double[]>> clipNotes = new HashMap<>();
        final Map<String, Double> clipLength = new HashMap<>();
        /** Whether Bitwig records a launcher clip into the arranger only on an armed track, or never. */
        boolean recordsOnlyArmed;
        boolean recordsNothing;
        /** Whether Bitwig ignores stopping a track, or arming one. */
        boolean trackStopIgnored;
        boolean armIgnored;
        /** Whether the notes a track plays never show, as on a track Bitwig might not say them for. */
        boolean notesHidden;
        /** Whether Bitwig's play position stays where it started while it plays (the link then goes by the clock). */
        boolean positionStill;
        /**
         * How long Bitwig's play position still says where it last stopped once it starts again, in ms: Bitwig 6.1.5 said
         * beat 24.2 (where the go before stopped) for at least 50 ms after starting a clip at beat 16.
         */
        long positionLag;
        /**
         * Whether Bitwig doesn't tell the link a plugin's parameter's new value once it has listed the device's parameters,
         * though their texts and the remote controls follow: Bitwig 6.1.5 with MO Probe, turned by the probe or by the link.
         */
        boolean pluginValuesUntold;
        /** Bitwig's "Write automation when recording Arranger clips": arranger automation write shows on while record is. */
        boolean automationWhileRecording;
        /** Whether new tracks get IDs ("new-1", "new-2"...), as Bitwig's do. */
        boolean newTrackIds;

        /**
         * Has Bitwig carry out the commands of its user interface only while it handles a hardware button, and (when
         * `sysex` is true) a SysEx message on the MIDI in, which may count as hardware too.
         */
        Model onlyHardware(boolean sysex) {
            honoured.clear();
            honoured.add(Context.HARDWARE);
            if (sysex) honoured.add(Context.SYSEX);
            return this;
        }

        Model tracks(TrackSpec... list) {
            tracks.addAll(Arrays.asList(list));
            return this;
        }
    }

    static final class TrackSpec {
        String name;
        /** Instrument, Audio, Group, Effect or Master; null for an instrument track. */
        String type;
        boolean group;
        boolean mute;
        float[] colour;
        Double volume;
        String volumeText;
        /** The ID Bitwig keeps the track under, or null where Bitwig hasn't said. */
        String id;
        /** Whether it's armed (extension 3.9). */
        boolean arm;
        /** Its launcher clip playing (or about to), or null; and whether it plays the launcher, not its arranger timeline. */
        Launched launched;
        boolean launcherMode;
        final List<DeviceSpec> devices = new ArrayList<>();
        /** Each clip slot's clip (the file it came from), or null where the slot is empty. */
        final List<String> slots = new ArrayList<>();
        /** The colours (#RRGGBB) of the launcher clips that have one of their own, by slot (see clipColour). */
        final Map<Integer, String> clipColours = new HashMap<>();

        TrackSpec(String name, String type) {
            this.name = name;
            this.type = type;
        }

        TrackSpec group() {
            group = true;
            return this;
        }

        TrackSpec colour(float r, float g, float b) {
            colour = new float[] {r, g, b};
            return this;
        }

        TrackSpec volume(double value, String text) {
            volume = value;
            volumeText = text;
            return this;
        }

        TrackSpec id(String id) {
            this.id = id;
            return this;
        }

        TrackSpec armed() {
            arm = true;
            return this;
        }

        TrackSpec slots(String... clips) {
            slots.addAll(Arrays.asList(clips));
            return this;
        }

        /** A fresh copy, as the stand-in project keeps it. */
        TrackSpec copy() {
            TrackSpec t = new TrackSpec(name, type);
            t.group = group;
            t.mute = mute;
            t.colour = colour == null ? null : colour.clone();
            t.volume = volume;
            t.volumeText = volumeText;
            t.id = id;
            t.arm = arm;
            for (DeviceSpec d : devices) t.devices.add(d.copy());
            t.slots.addAll(slots);
            t.clipColours.putAll(clipColours);
            return t;
        }
    }

    static final class DeviceSpec {
        String name;
        String preset;
        boolean plugin;
        /** Bitwig's word for what it does (note_to_audio...); null for audio_to_audio. */
        String kind;
        boolean enabled = true;
        /** The ID of a Bitwig device (the Sampler's, say). */
        String id;
        /** A Bitwig device's own parameters by ID (TRANSPOSE, COARSE), which the extension can set without selecting it. */
        Map<String, Double> params;
        /** Whether it's a Drum Machine, and what's on its pads, by note. */
        boolean drum;
        Map<Integer, PadSpec> pads;
        /** The parameters the extension sees once the device is selected. */
        List<Knob> knobs;
        /** The remote controls page Bitwig shows for it: its name, and the knobs on it (by ID). */
        String page;
        List<String> controls;
        /** The sample a Sampler plays. */
        String sample;

        DeviceSpec(String name) {
            this.name = name;
        }

        DeviceSpec preset(String preset) {
            this.preset = preset;
            return this;
        }

        DeviceSpec plugin() {
            plugin = true;
            return this;
        }

        DeviceSpec kind(String kind) {
            this.kind = kind;
            return this;
        }

        DeviceSpec disabled() {
            enabled = false;
            return this;
        }

        DeviceSpec id(String id) {
            this.id = id;
            return this;
        }

        DeviceSpec param(String param, double value) {
            if (params == null) params = new LinkedHashMap<>();
            params.put(param, value);
            return this;
        }

        DeviceSpec drum() {
            drum = true;
            return this;
        }

        DeviceSpec knobs(List<Knob> list) {
            knobs = new ArrayList<>(list);
            return this;
        }

        DeviceSpec knobs(Knob... list) {
            return knobs(Arrays.asList(list));
        }

        DeviceSpec page(String name, String... ids) {
            page = name;
            controls = Arrays.asList(ids);
            return this;
        }

        /** A copy through and through, as Bitwig makes a new device of what's inserted. */
        DeviceSpec copy() {
            DeviceSpec d = new DeviceSpec(name);
            d.preset = preset;
            d.plugin = plugin;
            d.kind = kind;
            d.enabled = enabled;
            d.id = id;
            d.params = params == null ? null : new LinkedHashMap<>(params);
            d.drum = drum;
            if (pads != null) {
                d.pads = new LinkedHashMap<>();
                for (Map.Entry<Integer, PadSpec> e : pads.entrySet()) d.pads.put(e.getKey(), e.getValue().copy());
            }
            if (knobs != null) {
                d.knobs = new ArrayList<>();
                for (Knob k : knobs) d.knobs.add(k.copy());
            }
            d.page = page;
            d.controls = controls == null ? null : new ArrayList<>(controls);
            d.sample = sample;
            return d;
        }
    }

    /** One of a device's parameters as the extension sees it once the device is selected. */
    static final class Knob {
        final String id;
        final String name;
        double value;
        /** The text Bitwig shows, or null for a parameter whose text never comes. */
        String display;
        /** Whether it stays put when set. */
        boolean locked;
        // What Bitwig says of it as a remote control or the last clicked parameter.
        int steps = -1;
        String[] stepNames = new String[0];
        double origin;
        boolean automated;

        Knob(String id, String name, double value, String display) {
            this.id = id;
            this.name = name;
            this.value = value;
            this.display = display;
        }

        Knob steps(String... names) {
            steps = names.length;
            stepNames = names.clone();
            return this;
        }

        Knob origin(double origin) {
            this.origin = origin;
            return this;
        }

        Knob automated() {
            automated = true;
            return this;
        }

        Knob copy() {
            Knob k = new Knob(id, name, value, display);
            k.locked = locked;
            k.steps = steps;
            k.stepNames = stepNames.clone();
            k.origin = origin;
            k.automated = automated;
            return k;
        }
    }

    static final class PadSpec {
        String name;
        final List<DeviceSpec> devices = new ArrayList<>();

        PadSpec(String name) {
            this.name = name;
        }

        PadSpec copy() {
            PadSpec p = new PadSpec(name);
            for (DeviceSpec d : devices) p.devices.add(d.copy());
            return p;
        }
    }

    record ActionSpec(String id, String name, String category, String menu) {}

    static TrackSpec track(String name, String type, DeviceSpec... devices) {
        TrackSpec t = new TrackSpec(name, type);
        t.devices.addAll(Arrays.asList(devices));
        return t;
    }

    static DeviceSpec device(String name) {
        return new DeviceSpec(name);
    }

    static Knob knob(String id, String name, double value, String display) {
        return new Knob(id, name, value, display);
    }

    static ActionSpec action(String id, String name, String category) {
        return new ActionSpec(id, name, category, null);
    }

    private static String baseName(String file) {
        String name = file.replace('\\', '/');
        name = name.substring(name.lastIndexOf('/') + 1);
        return name.replaceFirst("\\.[^.]*$", "");
    }

    private static String formatParam(String param, double v) {
        return param.equals("COARSE") ? (v > 0 ? "+" : "") + Json.number(v) + " st"
                                      : (v >= 0 ? "+" : "") + String.format(java.util.Locale.ROOT, "%.2f", v) + " st";
    }

    // ---- The parts of Bitwig ----

    /**
     * A part of the stand-in Bitwig. `api` is it as Bitwig's interfaces, and a call on that goes to this object's
     * method with the same name and number of arguments (a stand-in passed in arrives as itself). A stand-in
     * returned is handed over as its `api`, and a number as the type the API's method returns.
     */
    abstract static class Stand implements InvocationHandler {
        private static final ClassValue<Map<Method, Optional<Method>>> FOUND = new ClassValue<>() {
            @Override
            protected Map<Method, Optional<Method>> computeValue(Class<?> type) {
                return new ConcurrentHashMap<>();
            }
        };

        final Object api;

        Stand(Class<?>... types) {
            api = Proxy.newProxyInstance(FakeBitwig.class.getClassLoader(), types, this);
        }

        /** Runs before each call: a part of Bitwig that fails can throw here. */
        void calling(Method method) {}

        @Override
        public final Object invoke(Object proxy, Method method, Object[] args) throws Throwable {
            if (method.getDeclaringClass() == Object.class) {
                switch (method.getName()) {
                    case "equals": return proxy == args[0];
                    case "hashCode": return System.identityHashCode(proxy);
                    default: return "stand-in " + getClass().getSimpleName();
                }
            }
            calling(method);
            Method mine = FOUND.get(getClass()).computeIfAbsent(method, m -> Optional.ofNullable(find(getClass(), m))).orElse(null);
            if (mine == null)
                throw new UnsupportedOperationException("The stand-in Bitwig has no " + method.getDeclaringClass().getSimpleName() + "."
                                                        + method.getName() + " (asked of " + getClass().getName() + ").");
            Object[] given = args == null ? new Object[0] : args.clone();
            for (int i = 0; i < given.length; i++) given[i] = unwrap(given[i]);
            try {
                return returned(method.getReturnType(), mine.invoke(this, given));
            } catch (InvocationTargetException e) {
                throw e.getCause();
            }
        }

        private static Method find(Class<?> type, Method wanted) {
            for (Class<?> c = type; c != Stand.class && c != Object.class; c = c.getSuperclass()) {
                for (Method m : c.getDeclaredMethods()) {
                    if (!m.getName().equals(wanted.getName()) || m.getParameterCount() != wanted.getParameterCount()) continue;
                    if (m.isSynthetic() || Modifier.isStatic(m.getModifiers())) continue;
                    m.setAccessible(true);
                    return m;
                }
            }
            return null;
        }

        private static Object returned(Class<?> type, Object result) {
            if (result instanceof Stand) return ((Stand) result).api;
            if (type == boolean.class) return (Boolean) result;
            if (type == int.class) return ((Number) result).intValue();
            if (type == long.class) return ((Number) result).longValue();
            if (type == double.class) return ((Number) result).doubleValue();
            if (type == float.class) return ((Number) result).floatValue();
            return result;
        }

        /** The stand-in behind one of Bitwig's objects, or the value itself if it isn't one. */
        static Object unwrap(Object value) {
            if (value != null && Proxy.isProxyClass(value.getClass()) && Proxy.getInvocationHandler(value) instanceof Stand) {
                return Proxy.getInvocationHandler(value);
            }
            return value;
        }
    }

    /** A value the extension reads. Its set() goes through `onSet`, when given, so it can take effect later. */
    static class Value extends Stand {
        Object v;
        boolean interested;
        private final Consumer<Object> onSet;
        /** Where a value Bitwig keeps up to date by itself comes from (the recorder's duration), else null. */
        Supplier<Object> live;

        Value(Class<?> type, Object v) {
            this(type, v, null);
        }

        Value(Class<?> type, Object v, Consumer<Object> onSet) {
            super(type);
            this.v = v;
            this.onSet = onSet;
        }

        void markInterested() {
            interested = true;
        }

        Object get() {
            if (!interested) throw new IllegalStateException("read a value that wasn't marked as interested");
            return live != null ? live.get() : v;
        }

        void set(Object value) {
            if (onSet != null) onSet.accept(value);
            else v = value;
        }
    }

    /** A parameter: its value, its text as Bitwig shows it ("-6.0 dB"), and what else Bitwig says of it. */
    static class Param extends Value {
        final Value display;
        final Value present = new Value(BooleanValue.class, false);
        final Value title = new Value(SettableStringValue.class, "");
        final Value steps = new Value(IntegerValue.class, -1);
        final Value stepNames = new Value(StringArrayValue.class, new String[0]);
        final Value origin = new Value(DoubleValue.class, 0.0);
        final Value automated = new Value(BooleanValue.class, false);

        Param(Class<?> type, double v, String display) {
            super(type, v);
            this.display = new Value(StringValue.class, display);
        }

        Object getRaw() {
            return get();
        }

        Value displayedValue() {
            return display;
        }

        Value exists() {
            return present;
        }

        Value name() {
            return title;
        }

        Value discreteValueCount() {
            return steps;
        }

        Value discreteValueNames() {
            return stepNames;
        }

        Value getOrigin() {
            return origin;
        }

        Value hasAutomation() {
            return automated;
        }

        /** Shows a knob (or none), as a remote control or the last clicked parameter does. */
        void show(Knob knob) {
            present.v = knob != null;
            title.v = knob == null ? "" : knob.name;
            v = knob == null ? 0.0 : knob.value;
            display.v = knob == null || knob.display == null ? "" : knob.display;
            steps.v = knob == null ? -1 : knob.steps;
            stepNames.v = knob == null ? new String[0] : knob.stepNames.clone();
            origin.v = knob == null ? 0.0 : knob.origin;
            automated.v = knob != null && knob.automated;
        }
    }

    static final class Colour extends Stand {
        float r;
        float g;
        float b;
        private boolean interested;
        private final Consumer<float[]> onSet;

        Colour(Consumer<float[]> onSet) {
            super(SettableColorValue.class);
            this.onSet = onSet;
        }

        void markInterested() {
            interested = true;
        }

        private void checkInterested() {
            if (!interested) throw new IllegalStateException("read a colour that wasn't marked as interested");
        }

        float red() {
            checkInterested();
            return r;
        }

        float green() {
            checkInterested();
            return g;
        }

        float blue() {
            checkInterested();
            return b;
        }

        void set(float red, float green, float blue) {
            onSet.accept(new float[] {red, green, blue});
        }

        /** Shows a colour written #RRGGBB, or black for none, as Bitwig reports a colour. */
        void show(String hex) {
            int n = hex == null ? 0 : Integer.parseInt(hex.substring(1), 16);
            r = (n >> 16 & 0xFF) / 255f;
            g = (n >> 8 & 0xFF) / 255f;
            b = (n & 0xFF) / 255f;
        }
    }

    static class Bank extends Stand {
        final List<? extends Stand> items;
        final Value count = new Value(IntegerValue.class, 0);

        Bank(Class<?> type, List<? extends Stand> items) {
            super(type);
            this.items = items;
        }

        Stand getItemAt(int index) {
            return items.get(index);
        }

        Value itemCount() {
            return count;
        }
    }

    /** Logs an insertion point's calls, and hands each to `act` (if given) to carry out. */
    final class Point extends Stand {
        private final String where;
        private final BiConsumer<String, Object> act;

        Point(String where, BiConsumer<String, Object> act) {
            super(InsertionPoint.class);
            this.where = where;
            this.act = act;
        }

        void insertVST3Device(String id) {
            called("insertVST3Device", id);
        }

        void insertVST2Device(int id) {
            called("insertVST2Device", id);
        }

        void insertCLAPDevice(String id) {
            called("insertCLAPDevice", id);
        }

        void insertBitwigDevice(UUID id) {
            called("insertBitwigDevice", id);
        }

        void insertFile(String path) {
            called("insertFile", path);
        }

        private void called(String method, Object arg) {
            calls.add(where == null ? Arrays.asList(method, arg) : Arrays.asList(method, arg, where));
            if (act != null) act.accept(method, arg);
        }
    }

    /** A parameter of a specific Bitwig device: it exists while the device it looks at is that device. */
    static final class SpecificParam extends Param {
        private final FakeDevice owner;
        final String id;
        final String param;

        SpecificParam(FakeDevice owner, String id, String param) {
            super(Parameter.class, 0, "");
            this.owner = owner;
            this.id = id;
            this.param = param;
        }

        void setRaw(double value) {
            owner.setParam(id, param, value);
        }
    }

    class FakeDevice extends Stand {
        DeviceSpec spec;
        final List<SpecificParam> specifics = new ArrayList<>();
        final Value present = new Value(BooleanValue.class, false);
        final Value title = new Value(StringValue.class, "");
        final Value presetName = new Value(StringValue.class, "");
        final Value isPlugin = new Value(BooleanValue.class, false);
        final Value deviceType = new Value(EnumValue.class, "");
        final Value isEnabled = new Value(SettableBooleanValue.class, true);
        final Value sampleName = new Value(StringValue.class, "");

        FakeDevice() {
            this(Device.class);
        }

        FakeDevice(Class<?> type) {
            super(type);
        }

        void load(DeviceSpec spec) {
            this.spec = spec;
            present.v = spec != null;
            title.v = spec != null ? spec.name : "";
            presetName.v = spec != null && spec.preset != null ? spec.preset : "";
            isPlugin.v = spec != null && spec.plugin;
            deviceType.v = spec == null ? "" : spec.kind == null ? "audio_to_audio" : spec.kind;
            isEnabled.v = spec == null || spec.enabled;
            sampleName.v = spec != null && spec.sample != null ? spec.sample.substring(spec.sample.replace('\\', '/').lastIndexOf('/') + 1) : "";
            for (SpecificParam p : specifics) {
                boolean mine = spec != null && p.id.equals(spec.id);
                p.present.v = mine;
                p.v = mine && spec.params != null ? spec.params.getOrDefault(p.param, 0.0) : 0.0;
                p.display.v = mine ? formatParam(p.param, (Double) p.v) : "";
            }
        }

        Value exists() {
            return present;
        }

        Value name() {
            return title;
        }

        Value presetName() {
            return presetName;
        }

        Value isPlugin() {
            return isPlugin;
        }

        Value deviceType() {
            return deviceType;
        }

        Value isEnabled() {
            return isEnabled;
        }

        Value sampleName() {
            return sampleName;
        }

        Stand createSpecificBitwigDevice(UUID id) {
            FakeDevice owner = this;
            return new Stand(SpecificBitwigDevice.class) {
                Stand createParameter(String param) {
                    SpecificParam p = new SpecificParam(owner, id.toString(), param);
                    specifics.add(p);
                    return p;
                }
            };
        }

        /** Bitwig keeps a value in the parameter's range; the Sampler's real range isn't known, so ±48 stands in. */
        void setParam(String id, String param, double value) {
            DeviceSpec device = spec;
            later(20, () -> {
                if (device == null || !id.equals(device.id)) return;
                if (device.params == null) device.params = new LinkedHashMap<>();
                device.params.put(param, Math.max(-48, Math.min(48, param.equals("COARSE") ? Math.round(value) : value)));
            });
        }
    }

    /** The device selected in Bitwig, with Bitwig's "direct parameter" observers and its remote controls. */
    final class CursorDevice extends FakeDevice {
        /** The selected device's parameters. */
        List<Knob> params = new ArrayList<>();
        /** The parameters whose texts the extension asks for, or null for none. */
        String[] observed;
        final List<FakeDevice> selected = new ArrayList<>();
        final List<List<Object>> setCalls = new ArrayList<>();
        final Value where = new Value(IntegerValue.class, -1);
        final Value drum = new Value(BooleanValue.class, false);
        private StringArrayValueChangedCallback idObserver;
        private DirectParameterNameChangedCallback nameObserver;
        private DirectParameterNormalizedValueChangedCallback valueObserver;
        private DirectParameterDisplayedValueChangedCallback displayObserver;
        private boolean following;
        private DeviceSpec followed;
        Page page;

        CursorDevice() {
            super(PinnableCursorDevice.class);
        }

        void addDirectParameterIdObserver(StringArrayValueChangedCallback callback) {
            idObserver = callback;
        }

        void addDirectParameterNameObserver(int maxChars, DirectParameterNameChangedCallback callback) {
            nameObserver = callback;
        }

        void addDirectParameterNormalizedValueObserver(DirectParameterNormalizedValueChangedCallback callback) {
            valueObserver = callback;
        }

        Stand addDirectParameterValueDisplayObserver(int maxChars, DirectParameterDisplayedValueChangedCallback callback) {
            displayObserver = callback;
            return new Stand(DirectParameterValueDisplayObserver.class) {
                void setObservedParameterIds(String[] ids) {
                    observed = ids;
                    if (ids == null) return;
                    // Bitwig sends the texts a moment later; parameters without one never answer.
                    clock.schedule(() -> {
                        if (observed != ids) return;
                        List<String> wanted = Arrays.asList(ids);
                        for (Knob p : params)
                            if (p.display != null && wanted.contains(p.id)) displayObserver.directParameterDisplayedValueChanged(p.id, p.display);
                    }, 30);
                }
            };
        }

        /** Selects a device in "Bitwig" that isn't on any track: its parameters arrive through the observers. */
        void show(DeviceSpec spec, Knob... knobs) {
            cursorSpec = ALONE;
            following = false;
            load(spec);
            where.v = spec != null ? 0 : -1;
            drum.v = spec != null && spec.drum;
            announce(Arrays.asList(knobs));
            showPage();
        }

        /**
         * Bitwig's direct parameters for the device on a track the cursor is on now: the knobs its spec has, or
         * none. Only a different device announces them again.
         */
        void follow(DeviceSpec spec) {
            if (following && spec == followed) return;
            following = true;
            followed = spec;
            announce(spec != null && spec.knobs != null ? spec.knobs : List.of());
        }

        private void announce(List<Knob> knobs) {
            params = knobs;
            if (idObserver == null) return;   // the extension didn't watch them
            String[] ids = new String[knobs.size()];
            for (int i = 0; i < ids.length; i++) ids[i] = knobs.get(i).id;
            idObserver.valueChanged(ids);
            for (Knob p : knobs) {
                nameObserver.directParameterNameChanged(p.id, p.name);
                valueObserver.directParameterNormalizedValueChanged(p.id, p.value);
            }
        }

        /** The remote controls page shows the knobs the device's spec puts there. */
        void showPage() {
            if (page == null) return;
            List<Knob> on = new ArrayList<>();
            if (spec != null && spec.controls != null)
                for (String id : spec.controls)
                    for (Knob k : params)
                        if (k.id.equals(id)) on.add(k);
            page.show(spec == null || spec.page == null ? "" : spec.page, on);
        }

        Value position() {
            return where;
        }

        Value hasDrumPads() {
            return drum;
        }

        Stand createDrumPadBank(int n) {
            pads = new Pads(n);
            return pads;
        }

        Stand createCursorRemoteControlsPage(int n) {
            page = new Page(n);
            return page;
        }

        void selectDevice(FakeDevice device) {
            selected.add(device);
            FakeBitwig.this.selectDevice(device);
        }

        /** The value changes a moment later, and Bitwig tells the extension, as for a knob turned by hand. A locked knob stays put. */
        void setDirectParameterValueNormalized(String id, Number value, Number resolution) {
            setCalls.add(Arrays.asList(id, value.longValue(), resolution.longValue()));
            Knob knob = null;
            for (Knob k : params)
                if (k.id.equals(id)) knob = k;
            if (knob == null || knob.locked) return;
            double to = value.doubleValue() / (resolution.doubleValue() - 1);
            turn(knob, to, knob.display == null ? null : Math.round(to * 100) + "%");
        }

        /** The plugin turns one of its knobs itself, to a value from 0 to 1 that it shows as `display`. */
        void turnByPlugin(String id, double value, String display) {
            for (Knob k : params)
                if (k.id.equals(id)) turn(k, value, display);
        }

        private void turn(Knob knob, double value, String display) {
            later(20, () -> {
                knob.value = value;
                knob.display = display;
                if (!(model.pluginValuesUntold && spec != null && spec.plugin))
                    valueObserver.directParameterNormalizedValueChanged(knob.id, value);
                if (page != null) page.refresh();
            });
        }
    }

    /** A remote controls page: the controls Bitwig shows for the selected device. */
    static final class Page extends Stand {
        final Value title = new Value(StringValue.class, "");
        final List<Param> controls = new ArrayList<>();

        Page(int n) {
            super(CursorRemoteControlsPage.class);
            for (int i = 0; i < n; i++) controls.add(new Param(RemoteControl.class, 0, ""));
        }

        Value getName() {
            return title;
        }

        Param getParameter(int index) {
            return controls.get(index);
        }

        private List<Knob> shown = List.of();

        void show(String name, List<Knob> knobs) {
            title.v = name;
            shown = knobs;
            for (int i = 0; i < controls.size(); i++) controls.get(i).show(i < knobs.size() ? knobs.get(i) : null);
        }

        /** The controls follow their knobs, as Bitwig's do. */
        void refresh() {
            show((String) title.v, shown);
        }
    }

    final class Pad extends Stand {
        final int index;
        final Value present = new Value(BooleanValue.class, false);
        final Value title;
        /** The pad's devices, as many as the extension asks to see: the first can be replaced by a sample. */
        Bank chain = new Bank(DeviceBank.class, List.of());
        final Point point;

        Pad(int index) {
            super(DrumPad.class);
            this.index = index;
            title = new Value(SettableStringValue.class, "", name -> later(20, () -> {
                PadSpec pad = padAt(index);
                if (pad != null) pad.name = (String) name;
            }));
            point = new Point("pad " + index, (method, arg) -> {
                DeviceSpec device = padDevice();
                if (!method.equals("insertFile") || !AUDIO.matcher((String) arg).find() || device == null) return;
                later(40, () -> {
                    int note = padScroll + index;
                    if (note >= 128) return;
                    if (device.pads == null) device.pads = new LinkedHashMap<>();
                    device.pads.computeIfAbsent(note, n -> new PadSpec(baseName((String) arg)));
                    DeviceSpec sampler = builtIn.get(SAMPLER).copy();
                    sampler.sample = (String) arg;
                    device.pads.get(note).devices.add(sampler);
                });
            });
        }

        Value exists() {
            return present;
        }

        Value name() {
            return title;
        }

        Bank createDeviceBank(int n) {
            List<FakeDevice> devices = new ArrayList<>();
            devices.add(new PadDevice(index));
            for (int i = 1; i < n; i++) devices.add(new FakeDevice());
            chain = new Bank(DeviceBank.class, devices);
            return chain;
        }

        Point insertionPoint() {
            return point;
        }
    }

    /** A pad's first device. A sample put in its place takes the place of the pad's first device as a Sampler. */
    final class PadDevice extends FakeDevice {
        final Point replace;

        PadDevice(int index) {
            replace = new Point("pad " + index + " first device", (method, arg) -> {
                DeviceSpec device = padDevice();
                if (!method.equals("insertFile") || !AUDIO.matcher((String) arg).find() || device == null) return;
                later(40, () -> {
                    PadSpec pad = device.pads == null ? null : device.pads.get(padScroll + index);
                    if (pad == null || pad.devices.isEmpty()) return;
                    DeviceSpec sampler = builtIn.get(SAMPLER).copy();
                    sampler.sample = (String) arg;
                    pad.devices.set(0, sampler);
                });
            });
        }

        Point replaceDeviceInsertionPoint() {
            return replace;
        }
    }

    final class Pads extends Bank {
        boolean indication = true;
        final Value scroll = new Value(SettableIntegerValue.class, padScroll,
                                       v -> later(20, () -> padScroll = Math.max(0, Math.min(127, ((Number) v).intValue()))));

        Pads(int n) {
            super(DrumPadBank.class, new ArrayList<Pad>());
            @SuppressWarnings("unchecked")
            List<Pad> list = (List<Pad>) items;
            for (int i = 0; i < n; i++) list.add(new Pad(i));
        }

        void setIndication(boolean on) {
            indication = on;
        }

        Value scrollPosition() {
            return scroll;
        }
    }

    final class Slot extends Stand {
        final Value present = new Value(BooleanValue.class, false);
        final Value content = new Value(BooleanValue.class, false);
        final Value selected = new Value(BooleanValue.class, false);
        /** Whether its clip is playing, about to, or recording in the launcher (extension 3.9): never, here. */
        final Value playingClip = new Value(BooleanValue.class, false);
        final Value queued = new Value(BooleanValue.class, false);
        final Value recordingClip = new Value(BooleanValue.class, false);
        /** The clip's name: its file's, without the folder or the extension, as Bitwig names a clip made from a file. */
        final Value clipName = new Value(StringValue.class, "");
        /** The clip's colour. Setting it colours the clip a moment later, unless the model says Bitwig ignores that. */
        final Colour colour;
        private final int track;
        private final int scene;

        Slot(int track, int scene) {
            super(ClipLauncherSlot.class);
            this.track = track;
            this.scene = scene;
            playingClip.live = () -> clipPlaying(trackSpec(track), scene);
            queued.live = () -> clipQueued(trackSpec(track), scene);
            clipName.live = () -> {
                TrackSpec t = trackSpec(track);
                String file = t == null || scene >= t.slots.size() ? null : t.slots.get(scene);
                if (file == null) return "";
                String name = file.substring(Math.max(file.lastIndexOf('\\'), file.lastIndexOf('/')) + 1);
                return name.contains(".") ? name.substring(0, name.lastIndexOf('.')) : name;
            };
            colour = new Colour(rgb -> {
                calls.add(Arrays.asList("clipColour", track, scene, hex(rgb)));
                String made = hex(model.colourSnaps ? snapped(rgb) : rgb);
                later(20, () -> {
                    TrackSpec at = trackSpec(track);
                    if (model.colourIgnored || at == null || scene >= at.slots.size() || at.slots.get(scene) == null) return;
                    at.clipColours.put(scene, made);
                });
            });
        }

        /** Each of red, green and blue rounded to the nearest multiple of 0x33, as Bitwig's palette might. */
        private float[] snapped(float[] rgb) {
            return new float[] {Math.round(rgb[0] * 5) / 5f, Math.round(rgb[1] * 5) / 5f, Math.round(rgb[2] * 5) / 5f};
        }

        Colour color() {
            return colour;
        }

        Value exists() {
            return present;
        }

        Value hasContent() {
            return content;
        }

        Value isSelected() {
            return selected;
        }

        void select() {
            selectSlot(track, scene);
        }

        Value isPlaying() {
            return playingClip;
        }

        Value isPlaybackQueued() {
            return queued;
        }

        Value isRecording() {
            return recordingClip;
        }

        void launch() {
            FakeBitwig.this.launch(track, scene);
        }

        Value name() {
            return clipName;
        }

        /** Launched with options (extension 3.17 launches the Playground's clips "synced"): recorded, then launched as above. */
        void launchWithOptions(String quantization, String mode) {
            calls.add(Arrays.asList("launchWithOptions", track, scene, quantization, mode));
            FakeBitwig.this.launch(track, scene);
        }

        /** Deletes the slot's clip a moment later. */
        void deleteObject() {
            calls.add(Arrays.asList("deleteClip", track, scene));
            later(20, () -> {
                TrackSpec at = trackSpec(track);
                if (at == null || scene >= at.slots.size()) return;
                at.slots.set(scene, null);
                at.clipColours.remove(scene);
            });
        }

        Point replaceInsertionPoint() {
            return new Point("track " + track + " slot " + scene, (method, arg) -> {
                if (!method.equals("insertFile") || !MIDI.matcher((String) arg).find()) return;
                TrackSpec at = state.tracks.get(track);
                later(model.clipDelay, () -> {
                    while (at.slots.size() <= scene) at.slots.add(null);
                    at.slots.set(scene, (String) arg);
                    at.clipColours.remove(scene);   // a new clip, in its track's colour
                    state.scenes = Math.max(state.scenes, scene + 1);
                });
            });
        }
    }

    /** A track in the bank (index 0 up), or the cursor track (index -1). */
    final class FakeTrack extends Stand {
        final int index;
        final Value present = new Value(BooleanValue.class, false);
        final Value title;
        final Value trackType = new Value(StringValue.class, "");
        final Value isGroup = new Value(BooleanValue.class, false);
        final Value isActivated = new Value(SettableBooleanValue.class, true);
        final Value mute = new Value(SettableBooleanValue.class, false);
        final Value solo = new Value(SoloValue.class, false);
        final Value arm;
        final Value channelId = new Value(StringValue.class, "");
        /** Whether no launcher clip plays on it, and the notes it plays (extension 3.9), as they are whenever the extension looks. */
        final Value stopped = new Value(BooleanValue.class, true);
        final Value notes = new Value(PlayingNoteArrayValue.class, new PlayingNote[0]);
        final Colour colour;
        final Param volume = new Param(Parameter.class, 0.79, "0.0 dB");
        final Param pan = new Param(Parameter.class, 0.5, "C");
        Bank devices;
        Bank slots;
        Value isCursor;
        private final Point start;
        private final Point end;

        FakeTrack(int index) {
            super(index < 0 ? CursorTrack.class : Track.class);
            this.index = index;
            title = new Value(SettableStringValue.class, "", whenSet((spec, v) -> spec.name = (String) v));
            colour = new Colour(whenSet((spec, rgb) -> spec.colour = (float[]) rgb));
            Consumer<Object> arming = whenSet((spec, v) -> {
                if (!model.armIgnored) spec.arm = (Boolean) v;
            });
            arm = new Value(SettableBooleanValue.class, false, v -> {
                calls.add(Arrays.asList("arm", index, v));
                arming.accept(v);
            });
            stopped.live = () -> !clipPlaying(trackSpec(index), null);
            notes.live = () -> notesOf(trackSpec(index));
            start = index < 0 ? null : chain("start");
            end = index < 0 ? new Point(null, null) : chain("end");
        }

        /** What setting one of the track's values does: it changes the project's track a moment later. */
        private <T> Consumer<T> whenSet(BiConsumer<TrackSpec, Object> apply) {
            return v -> later(20, () -> {
                TrackSpec spec = index < 0 ? cursorSpec() : trackSpec(index);
                if (spec != null) apply.accept(spec, v);
            });
        }

        private Point chain(String at) {
            return new Point("track " + index + " " + at, (method, arg) -> insert(index, at, method, arg));
        }

        void load(TrackSpec spec) {
            present.v = spec != null;
            if (isCursor != null) isCursor.v = index >= 0 && cursorIndex == index;
            if (spec == null) return;
            title.v = spec.name;
            trackType.v = spec.type == null ? "Instrument" : spec.type;
            isGroup.v = spec.group;
            mute.v = spec.mute;
            arm.v = spec.arm;
            channelId.v = spec.id == null ? "" : spec.id;
            if (spec.colour != null) {
                colour.r = spec.colour[0];
                colour.g = spec.colour[1];
                colour.b = spec.colour[2];
            }
            if (spec.volume != null) {
                volume.v = spec.volume;
                volume.display.v = spec.volumeText;
            }
            if (devices != null) {
                for (int i = 0; i < devices.items.size(); i++)
                    ((FakeDevice) devices.items.get(i)).load(i < spec.devices.size() ? spec.devices.get(i) : null);
                devices.count.v = spec.devices.size();
            }
            if (slots != null) {
                for (int s = 0; s < slots.items.size(); s++) {
                    Slot slot = (Slot) slots.items.get(s);
                    slot.present.v = s < state.scenes;
                    boolean clip = s < spec.slots.size() && spec.slots.get(s) != null;
                    slot.content.v = clip;
                    slot.colour.show(clip ? clipColour(spec, s) : null);
                    slot.selected.v = slotSelected != null && slotSelected[0] == index && slotSelected[1] == s;
                }
            }
        }

        Value exists() {
            return present;
        }

        Value name() {
            return title;
        }

        Value trackType() {
            return trackType;
        }

        Value isGroup() {
            return isGroup;
        }

        Value isActivated() {
            return isActivated;
        }

        Value mute() {
            return mute;
        }

        Value solo() {
            return solo;
        }

        Value arm() {
            return arm;
        }

        Value channelId() {
            return channelId;
        }

        Value isStopped() {
            return stopped;
        }

        Value playingNotes() {
            return notes;
        }

        void stop() {
            stopTrack(index);
        }

        void returnToArrangement() {
            FakeBitwig.this.returnToArrangement(index);
        }

        Colour color() {
            return colour;
        }

        Param volume() {
            return volume;
        }

        Param pan() {
            return pan;
        }

        Bank createDeviceBank(int n) {
            List<FakeDevice> list = new ArrayList<>();
            for (int i = 0; i < n; i++) list.add(new FakeDevice());
            devices = new Bank(DeviceBank.class, list);
            return devices;
        }

        Value createEqualsValue(Object other) {
            if (other != cursorTrack) throw new IllegalStateException("expected a comparison with the cursor track");
            isCursor = new Value(BooleanValue.class, false);
            return isCursor;
        }

        Point startOfDeviceChainInsertionPoint() {
            return start;
        }

        Point endOfDeviceChainInsertionPoint() {
            return end;
        }

        Point afterTrackInsertionPoint() {
            return new Point("after track " + index, (method, arg) -> fileAfter(index, method, arg));
        }

        Bank clipLauncherSlotBank() {
            List<Slot> list = new ArrayList<>();
            for (int s = 0; s < bankScenes; s++) list.add(new Slot(index, s));
            slots = new Bank(ClipLauncherSlotBank.class, list);
            return slots;
        }

        // The cursor track's own.

        void selectChannel(FakeTrack track) {
            calls.add(Arrays.asList("selectChannel", track));
            selectTrack(track);
        }

        CursorDevice createCursorDevice() {
            return cursorDevice;
        }
    }

    /** The parameter last clicked in Bitwig. */
    final class LastClicked extends Stand {
        final Param parameter = new Param(Parameter.class, 0, "");
        final Value locked = new Value(SettableBooleanValue.class, false);

        LastClicked() {
            super(LastClickedParameter.class);
        }

        Param parameter() {
            return parameter;
        }

        Value isLocked() {
            return locked;
        }
    }

    /** Bitwig's master recorder: it records once the audio engine is on, and counts how long it has. */
    final class Recorder extends Stand {
        final Value active = new Value(BooleanValue.class, false);
        final Value duration = new Value(IntegerValue.class, 0);
        boolean recording;
        private long startedAt;
        private long recorded;

        Recorder() {
            super(MasterRecorder.class);
            duration.live = () -> (int) (recording ? clock.now - startedAt : recorded);
        }

        Value isActive() {
            return active;
        }

        Value duration() {
            return duration;
        }

        void start() {
            calls.add(List.of("startRecording"));
            later(20, () -> {
                if (recording || !(Boolean) engine.v) return;
                recording = true;
                startedAt = clock.now;
            });
        }

        void stop() {
            calls.add(List.of("stopRecording"));
            later(20, () -> {
                if (!recording) return;
                recording = false;
                recorded = clock.now - startedAt;
            });
        }
    }

    /** The arranger's cursor clip: the clip selected in the arranger, its colour, and the track it's on. */
    static final class ArrangerClip extends Stand {
        final Value present = new Value(BooleanValue.class, false);
        final Colour colour = new Colour(rgb -> {
            throw new IllegalStateException("The link only reads the arranger clip's colour.");
        });
        final ClipTrack track = new ClipTrack();

        ArrangerClip() {
            super(Clip.class);
        }

        Value exists() {
            return present;
        }

        Colour color() {
            return colour;
        }

        ClipTrack getTrack() {
            return track;
        }
    }

    /** The track a clip is on, as the clip shows it, and (once the extension asked) whether it's the selected track. */
    static final class ClipTrack extends Stand {
        final Value present = new Value(BooleanValue.class, false);
        final Value title = new Value(SettableStringValue.class, "");
        Value isCursor;

        ClipTrack() {
            super(Track.class);
        }

        Value exists() {
            return present;
        }

        Value name() {
            return title;
        }

        Value createEqualsValue(Object other) {
            if (!(other instanceof FakeTrack) || ((FakeTrack) other).index >= 0)
                throw new IllegalStateException("expected a comparison with the cursor track");
            isCursor = new Value(BooleanValue.class, false);
            return isCursor;
        }
    }

    /**
     * A cursor clip with a note grid (extension 3.15): the clip selected in the arranger or the clip launcher, its loop,
     * its track, and the steps of its grid. As Bitwig's does, it tells the extension's observers each step of the grid
     * that changes, for each channel: NoteOn where a note starts (the step its start falls in), NoteSustain where one goes
     * on, and Empty where a step it told of before has no note any more.
     */
    static final class GridClip extends Stand {
        final Value present = new Value(BooleanValue.class, false);
        final Value loopStart = new Value(SettableBeatTimeValue.class, 0.0);
        final Value loopLength = new Value(SettableBeatTimeValue.class, 0.0);
        final ClipTrack track = new ClipTrack();
        final int width;
        final int height;
        /** The grid's step in beats (Bitwig's default is a 16th note), and the step and key it starts from. */
        double stepSize = 0.25;
        int firstStep;
        int firstKey;
        private final List<Object> observers = new ArrayList<>();
        /** What the observers were told last of each step, by channel, step and key, apart from Empty. */
        private final Map<List<Integer>, Told> told = new HashMap<>();

        private record Told(String state, ClipNote note) {}

        GridClip(int width, int height) {
            super(Clip.class);
            this.width = width;
            this.height = height;
        }

        Value exists() {
            return present;
        }

        Value getLoopStart() {
            return loopStart;
        }

        Value getLoopLength() {
            return loopLength;
        }

        ClipTrack getTrack() {
            return track;
        }

        void setStepSize(double size) {
            stepSize = size;
        }

        void scrollToKey(int key) {
            firstKey = key;
        }

        void scrollToStep(int step) {
            firstStep = step;
        }

        void addNoteStepObserver(Object callback) {
            observers.add(callback);
        }

        /** Shows a clip (or none) as selected, on the track called `cursorTrack` being the selected one or not. */
        void show(NoteClip clip, String cursorTrack) {
            present.v = clip != null;
            loopStart.v = clip == null ? 0.0 : clip.start;
            loopLength.v = clip == null ? 0.0 : clip.length;
            track.present.v = clip != null;
            track.title.v = clip == null ? "" : clip.track;
            if (track.isCursor != null) track.isCursor.v = clip != null && clip.track.equals(cursorTrack);

            Map<List<Integer>, Told> now = new HashMap<>();
            if (clip != null) {
                for (ClipNote n : clip.notes) {
                    int first = (int) Math.floor(n.start() / stepSize + 1e-9);
                    int last = (int) Math.ceil((n.start() + n.length()) / stepSize - 1e-9) - 1;
                    for (int step = first; step <= Math.max(first, last); step++) {
                        int x = step - firstStep;
                        int y = n.key() - firstKey;
                        if (x < 0 || x >= width || y < 0 || y >= height) continue;
                        List<Integer> at = List.of(n.channel(), x, y);
                        if (step == first) now.put(at, new Told("NoteOn", n));
                        else now.putIfAbsent(at, new Told("NoteSustain", n));
                    }
                }
            }
            for (List<Integer> at : new ArrayList<>(told.keySet())) {
                if (now.containsKey(at)) continue;
                told.remove(at);
                tell(at, new Told("Empty", null));
            }
            for (Map.Entry<List<Integer>, Told> e : now.entrySet()) {
                if (e.getValue().equals(told.get(e.getKey()))) continue;
                told.put(e.getKey(), e.getValue());
                tell(e.getKey(), e.getValue());
            }
        }

        private void tell(List<Integer> at, Told what) {
            Object step = noteStep(at.get(0), at.get(1), at.get(2), what.state(), what.note());
            for (Object callback : observers) callBack(callback, step);
        }
    }

    /** One step of a clip's note grid, as Bitwig hands it to a note step observer. */
    private static Object noteStep(int channel, int x, int y, String state, ClipNote note) {
        Object stateConstant = apiEnum(NoteStep.class, "state", state);
        return new Stand(NoteStep.class) {
            int channel() {
                return channel;
            }

            int x() {
                return x;
            }

            int y() {
                return y;
            }

            Object state() {
                return stateConstant;
            }

            double duration() {
                return note == null ? 0.0 : note.length();
            }

            double velocity() {
                return note == null ? 0.0 : note.velocity();
            }
        }.api;
    }

    /** The constant called `name` of the enum an API method returns (NoteStep.state()'s), found by the method. */
    private static Object apiEnum(Class<?> type, String method, String name) {
        try {
            for (Object constant : type.getMethod(method).getReturnType().getEnumConstants())
                if (((Enum<?>) constant).name().equals(name)) return constant;
        } catch (NoSuchMethodException e) {
            throw new IllegalStateException(e);
        }
        throw new IllegalStateException(type.getSimpleName() + "." + method + "() has no " + name + ".");
    }

    /**
     * Calls a callback the extension handed Bitwig, with one argument, whatever the API calls its interface: by the one
     * abstract method of the interface the extension's lambda implements.
     */
    private static void callBack(Object callback, Object arg) {
        for (Class<?> type : callback.getClass().getInterfaces()) {
            for (Method m : type.getMethods()) {
                if (!Modifier.isAbstract(m.getModifiers()) || m.getParameterCount() != 1) continue;
                try {
                    m.invoke(callback, arg);
                } catch (IllegalAccessException e) {
                    throw new IllegalStateException(e);
                } catch (InvocationTargetException e) {
                    if (e.getCause() instanceof RuntimeException) throw (RuntimeException) e.getCause();
                    if (e.getCause() instanceof Error) throw (Error) e.getCause();
                    throw new IllegalStateException(e.getCause());
                }
                return;
            }
        }
        throw new IllegalStateException("The extension's callback has no method to call: " + callback);
    }

    private Stand action(ActionSpec a) {
        return new Stand(Action.class) {
            String getId() {
                return a.id();
            }

            String getName() {
                return a.name();
            }

            String getMenuItemText() {
                return a.menu() != null ? a.menu() : a.name();
            }

            Stand getCategory() {
                if (a.category() == null) return null;
                return new Stand(ActionCategory.class) {
                    String getName() {
                        return a.category();
                    }
                };
            }

            /**
             * Runs the action. Save, Copy, Paste and the focus actions are commands of Bitwig's user interface, which it
             * carries out only where the model says: Save leaves the project without unsaved changes a moment later.
             */
            void invoke() {
                calls.add(Arrays.asList("invoke", a.id()));
                switch (a.id()) {
                    case Link.FOCUS_ARRANGER, Link.FOCUS_LAUNCHER -> {
                        if (honoured(a.id())) focusOrToggle(a.id());
                    }
                    case "Save" -> {
                        if (honoured("Save")) later(20, () -> modified.v = false);
                    }
                    case "Copy" -> {
                        if (honoured("Copy")) copied();
                    }
                    case "Paste" -> {
                        if (honoured("Paste")) pasted();
                    }
                    case "Duplicate" -> {
                        if (honoured("Duplicate")) duplicated();
                    }
                    case Link.TOGGLE_ARRANGER -> {
                        if (honoured(a.id())) toggleArranger();
                    }
                    default -> {}
                }
            }
        };
    }

    // ---- MIDI and hardware (the routes) ----

    /** The extension's MIDI in: it takes a SysEx callback and makes the matchers its buttons listen with. */
    private final Stand midiIn = new Stand(MidiIn.class) {
        void setSysexCallback(SysexMidiDataReceivedCallback callback) {
            sysexCallback = callback;
        }

        Matcher createNoteOnActionMatcher(int channel, int note) {
            return new Matcher(channel, note);
        }
    };

    /**
     * The extension's MIDI out. What it sends goes in `midi`, and when the model loops it back, comes in again a moment
     * later: a SysEx message to the SysEx callback, a note on to the buttons listening for it.
     */
    private final Stand midiOut = new Stand(MidiOut.class) {
        void sendSysex(Object data) {
            String hex = data instanceof byte[] bytes ? hexBytes(bytes) : (String) data;
            midi.add(Arrays.asList("sysex", hex));
            if (!loopsBack()) return;
            // As Bitwig hands it over: lower case, without spaces.
            String back = hex.replace(" ", "").toLowerCase(Locale.ROOT);
            clock.schedule(() -> {
                if (sysexCallback != null) in(Context.SYSEX, () -> sysexCallback.sysexDataReceived(back));
            }, model.loopbackDelay);
        }

        void sendMidi(int status, int data1, int data2) {
            midi.add(Arrays.asList("midi", status, data1, data2));
            if (!loopsBack() || (status & 0xF0) != 0x90 || data2 == 0) return;
            clock.schedule(() -> press(status & 0x0F, data1), model.loopbackDelay);
        }
    };

    private boolean loopsBack() {
        return model.loopback && (model.loopbackCut == null || !model.loopbackCut.getAsBoolean());
    }

    private static String hexBytes(byte[] bytes) {
        StringBuilder text = new StringBuilder();
        for (byte b : bytes) text.append(String.format("%02X", b & 0xFF));
        return text.toString();
    }

    /** A note on `note` (0 up) on MIDI channel `channel` (0 to 15) came in: presses each button listening for it. */
    void press(int channel, int note) {
        for (Button button : new ArrayList<>(buttons)) {
            Matcher m = button.pressed.matcher;
            if (m != null && m.channel == channel && m.note == note) in(Context.HARDWARE, button.pressed::run);
        }
    }

    /** Presses the extension's button called `id` (as made with createHardwareButton), as a note coming in would. */
    void pressButton(String id) {
        for (Button button : new ArrayList<>(buttons))
            if (button.id.equals(id)) in(Context.HARDWARE, button.pressed::run);
    }

    /** Sends a SysEx message (as hex) to the extension's MIDI in, as one coming back through the loopback would. */
    void sysexIn(String hex) {
        in(Context.SYSEX, () -> sysexCallback.sysexDataReceived(hex));
    }

    /** What a button listens for: a note on, on a channel (0 to 15). */
    static final class Matcher extends Stand {
        final int channel;
        final int note;

        Matcher(int channel, int note) {
            super(HardwareActionMatcher.class);
            this.channel = channel;
            this.note = note;
        }
    }

    /** One of the extension's buttons: its ID, and what pressing it does. */
    final class Button extends Stand {
        final String id;
        final Press pressed = new Press();

        Button(String id) {
            super(HardwareButton.class);
            this.id = id;
        }

        Press pressedAction() {
            return pressed;
        }
    }

    /** A button's press: the matcher it listens with, and what it's bound to, which a press runs in order. */
    final class Press extends Stand {
        Matcher matcher;
        final List<Object> bindings = new ArrayList<>();

        Press() {
            super(HardwareAction.class);
        }

        void setActionMatcher(Matcher m) {
            matcher = m;
        }

        Stand addBinding(Object bindable) {
            bindings.add(bindable);
            return binding(bindable);
        }

        Stand setBinding(Object bindable) {
            bindings.clear();
            return addBinding(bindable);
        }

        void clearBindings() {
            bindings.clear();
        }

        private Stand binding(Object bindable) {
            return new Stand(HardwareBinding.class) {
                void removeBinding() {
                    bindings.remove(bindable);
                }
            };
        }

        /** A press: runs each of Bitwig's actions bound, and each of the extension's own (made with createAction). */
        void run() {
            for (Object bound : new ArrayList<>(bindings)) {
                if (bound instanceof Callback callback) callback.run.run();
                else ((HardwareActionBindable) ((Stand) bound).api).invoke();
            }
        }
    }

    /** One of the extension's own actions (made with createAction), which a button runs when it's bound to it. */
    static final class Callback extends Stand {
        final Runnable run;
        final String name;

        Callback(Runnable run, String name) {
            super(HardwareActionBindable.class);
            this.run = run;
            this.name = name;
        }
    }

    /** Bitwig itself, as the extension's ControllerHost. */
    final class Host extends Stand {
        private final Stand application = new Stand(Application.class) {
            Value projectName() {
                return projectName;
            }

            Value hasActiveEngine() {
                return engine;
            }

            Action[] getActions() {
                Action[] list = new Action[actions.size()];
                for (int i = 0; i < list.length; i++) list[i] = (Action) actions.get(i).api;
                return list;
            }

            Stand getAction(String id) {
                for (Stand a : actions)
                    if (((Action) a.api).getId().equals(id)) return a;
                return null;
            }

            void createInstrumentTrack(int position) {
                FakeBitwig.this.createInstrumentTrack(position);
            }

            Value panelLayout() {
                return layout;
            }

            void setPanelLayout(String name) {
                calls.add(Arrays.asList("setPanelLayout", name));
                later(20, () -> {
                    if (!model.layoutIgnored) layoutNow = name;
                });
            }

            void copy() {
                FakeBitwig.this.copy();
            }

            void paste() {
                FakeBitwig.this.paste();
            }
        };

        private final Value armed = new Value(BooleanValue.class, false);

        private final Stand project = new Stand(Project.class) {
            Value isModified() {
                return modified;
            }

            Value hasArmedTracks() {
                armed.live = () -> state.tracks.stream().anyMatch(t -> t.arm);
                return armed;
            }

            void unarmAll() {
                calls.add(List.of("unarmAll"));
                later(20, () -> {
                    if (!model.armIgnored) state.tracks.forEach(t -> t.arm = false);
                });
            }
        };

        private final Stand transport = new Stand(Transport.class) {
            Param tempo() {
                return tempo;
            }

            Value timeSignature() {
                return signature;
            }

            Value isPlaying() {
                return playing;
            }

            Value getPosition() {
                return position;
            }

            Value playStartPosition() {
                return playStart;
            }

            // Extension 3.9.
            Value isArrangerRecordEnabled() {
                return transportValues.get("isArrangerRecordEnabled");
            }

            Value isMetronomeEnabled() {
                return transportValues.get("isMetronomeEnabled");
            }

            Value isArrangerLoopEnabled() {
                return transportValues.get("isArrangerLoopEnabled");
            }

            Value isPunchInEnabled() {
                return transportValues.get("isPunchInEnabled");
            }

            Value isPunchOutEnabled() {
                return transportValues.get("isPunchOutEnabled");
            }

            Value isArrangerOverdubEnabled() {
                return transportValues.get("isArrangerOverdubEnabled");
            }

            Value isClipLauncherOverdubEnabled() {
                return transportValues.get("isClipLauncherOverdubEnabled");
            }

            Value isArrangerAutomationWriteEnabled() {
                return transportValues.get("isArrangerAutomationWriteEnabled");
            }

            Value isClipLauncherAutomationWriteEnabled() {
                return transportValues.get("isClipLauncherAutomationWriteEnabled");
            }

            Value automationWriteMode() {
                return transportValues.get("automationWriteMode");
            }

            Value preRoll() {
                return transportValues.get("preRoll");
            }

            Value defaultLaunchQuantization() {
                return transportValues.get("defaultLaunchQuantization");
            }

            Value playPosition() {
                return playPosition;
            }

            /** Bitwig starts playing from the play start, a moment later. */
            void play() {
                calls.add(List.of("play"));
                int stops = FakeBitwig.this.stops;
                startsDue++;
                later(model.startDelay, () -> {
                    startsDue--;
                    if (stops == FakeBitwig.this.stops && !playingNow) startPlaying(playStartAt);
                });
            }

            /** Bitwig stops, a moment later; Stop while it's stopped moves the play start to the beginning, as in Bitwig. */
            void stop() {
                calls.add(List.of("stop"));
                later(20, () -> {
                    if (playingNow) {
                        stopPlaying();
                    } else if (startsDue > 0) {
                        // Bitwig takes commands in order: a launch or Play asked for before this Stop starts, then stops.
                        stops++;
                    } else {
                        calls.add(List.of("stopWhileStopped"));
                        playStartAt = 0;
                    }
                });
            }
        };

        private final Stand arranger = new Stand(Arranger.class) {
            Value isTimelineVisible() {
                return timeline;
            }

            Value isClipLauncherVisible() {
                return launcher;
            }
        };

        private final Stand browser = new Stand(PopupBrowser.class) {
            Value exists() {
                return browserOpen;
            }

            void cancel() {
                calls.add(List.of("cancelBrowser"));
                browserOpen.v = false;
            }
        };

        Host() {
            super(ControllerHost.class);
        }

        @Override
        void calling(Method method) {
            // As in a Bitwig without this part of the API: Java's error for a method the host doesn't have.
            if (model.broken.contains(method.getName()))
                throw new AbstractMethodError("The stand-in Bitwig has no " + method.getName() + ", as asked.");
        }

        Stand createApplication() {
            return application;
        }

        Stand getProject() {
            return project;
        }

        Stand createTransport() {
            return transport;
        }

        Bank createTrackBank(int n, int sends, int scenes, boolean flat) {
            if (!flat) throw new IllegalStateException("expected a flat track list");
            bankScenes = scenes;
            for (int i = 0; i < n; i++) trackItems.add(new FakeTrack(i));
            tracks = new Bank(TrackBank.class, trackItems);
            return tracks;
        }

        FakeTrack createCursorTrack(String id, String name, int sends, int scenes, boolean follow) {
            if (!follow) throw new IllegalStateException("expected a cursor that follows the selection");
            return cursorTrack;
        }

        Stand createPopupBrowser() {
            return browser;
        }

        Stand getPreferences() {
            return new Stand(Preferences.class) {
                Value getBooleanSetting(String label, String category, boolean initial) {
                    changes = new Value(SettableBooleanValue.class, initial);
                    return changes;
                }
            };
        }

        Stand createRemoteConnection(String name, int port) {
            return new Stand(RemoteSocket.class) {
                void setClientConnectCallback(ConnectionEstablishedCallback callback) {
                    connect = callback;
                }

                int getPort() {
                    if (model.portUnknown > 0) {
                        model.portUnknown--;
                        return -1;
                    }
                    return model.portInUse != 0 ? model.portInUse : port;
                }
            };
        }

        LastClicked createLastClickedParameter(String id, String name) {
            return lastClicked;
        }

        Recorder createMasterRecorder() {
            return recorder;
        }

        Stand createArranger() {
            return arranger;
        }

        Stand createMasterTrack(int scenes) {
            return master;
        }

        /** The arranger feature's clip has a grid of one step, which shows no notes; notes' has a note grid (extension 3.15). */
        Stand createArrangerCursorClip(int gridWidth, int gridHeight) {
            if (gridWidth <= 1 && gridHeight <= 1) return arrangerClip;
            arrangerGrid = new GridClip(gridWidth, gridHeight);
            return arrangerGrid;
        }

        GridClip createLauncherCursorClip(int gridWidth, int gridHeight) {
            launcherGrid = new GridClip(gridWidth, gridHeight);
            return launcherGrid;
        }

        void showPopupNotification(String text) {
            popups.add(text);
        }

        void println(String text) {
            printed.add(text);
        }

        void errorln(String text) {
            errors.add(text);
        }

        String getHostProduct() {
            return "Bitwig Studio";
        }

        String getHostVersion() {
            return model.version;
        }

        int getHostApiVersion() {
            return model.api;
        }

        PlatformType getPlatformType() {
            return PlatformType.WINDOWS;
        }

        void scheduleTask(Runnable task, long ms) {
            clock.schedule(() -> in(Context.TASK, task), ms);
            waiting();
        }

        Stand getMidiInPort(int index) {
            if (index != 0) throw new IllegalArgumentException("The extension has one MIDI in, not " + (index + 1) + ".");
            return midiIn;
        }

        Stand getMidiOutPort(int index) {
            if (index != 0) throw new IllegalArgumentException("The extension has one MIDI out, not " + (index + 1) + ".");
            return midiOut;
        }

        Stand createHardwareSurface() {
            return new Stand(HardwareSurface.class) {
                void setPhysicalSize(double width, double height) {
                    surfaceSize = new double[] {width, height};
                }

                Button createHardwareButton(String id) {
                    Button button = new Button(id);
                    buttons.add(button);
                    return button;
                }
            };
        }

        @SuppressWarnings("unchecked")
        Callback createAction(Object run, Supplier<?> name) {
            return new Callback((Runnable) run, String.valueOf(name.get()));
        }

        /** Bitwig calls the extension's flush a moment later (unless the model says it never does). */
        void requestFlush() {
            if (model.noFlush) return;
            clock.schedule(() -> in(Context.FLUSH, link::flush), 0);
        }
    }

    /** The stand-in's clock: time passes only when a test says so, and scheduled tasks run as it does. */
    static final class FakeClock extends Clock {
        /** When the stand-in's time starts (UTC), which names the report files. */
        static final Instant START = Instant.parse("2026-10-14T09:52:42Z");
        long now;
        private final List<Task> tasks = new ArrayList<>();
        private long added;

        void schedule(Runnable task, long ms) {
            tasks.add(new Task(now + ms, added++, task));
        }

        /** Lets `ms` pass, running each task when its time comes (in the order they were scheduled, for the same time). */
        void advance(long ms) {
            long end = now + ms;
            while (true) {
                Task next = null;
                for (Task t : tasks)
                    if (next == null || t.at < next.at || (t.at == next.at && t.order < next.order)) next = t;
                if (next == null || next.at > end) break;
                tasks.remove(next);
                now = next.at;
                next.run.run();
            }
            now = end;
        }

        @Override
        public ZoneId getZone() {
            return ZoneOffset.UTC;
        }

        @Override
        public Clock withZone(ZoneId zone) {
            return this;
        }

        @Override
        public Instant instant() {
            return START.plusMillis(now);
        }

        @Override
        public long millis() {
            return START.toEpochMilli() + now;
        }

        private record Task(long at, long order, Runnable run) {}
    }
}
