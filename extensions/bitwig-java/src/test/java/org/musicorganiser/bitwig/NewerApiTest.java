package org.musicorganiser.bitwig;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.musicorganiser.bitwig.FakeBitwig.device;
import static org.musicorganiser.bitwig.FakeBitwig.knob;
import static org.musicorganiser.bitwig.Messages.assertJson;
import static org.musicorganiser.bitwig.Messages.assertMatches;
import static org.musicorganiser.bitwig.Messages.at;
import static org.musicorganiser.bitwig.Messages.error;
import static org.musicorganiser.bitwig.Messages.list;
import static org.musicorganiser.bitwig.Messages.listAt;
import static org.musicorganiser.bitwig.Messages.obj;

import java.nio.file.Path;
import java.util.Arrays;
import java.util.List;
import java.util.Map;
import java.util.stream.Collectors;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;
import org.musicorganiser.bitwig.FakeBitwig.App;
import org.musicorganiser.bitwig.FakeBitwig.Model;
import org.musicorganiser.bitwig.FakeBitwig.NoteClip;

/** What link version 3 adds with Bitwig's newer API: track IDs, parameter steps and more, clicked, recorder and notes. */
class NewerApiTest {
    @TempDir
    Path reports;

    FakeBitwig bitwig;
    App app;

    void start(Model model) {
        bitwig = FakeBitwig.start(model, reports);
        app = bitwig.connect();
        app.take();
    }

    @Test
    @DisplayName("gives a track the ID Bitwig keeps it under, once Bitwig has said")
    void tracksCarryTheirIds() {
        Model m = LinkTest.song();
        m.tracks.get(1).id("a3f0c1d2-2b7e-4c55-9b1e-6d0f2c8e4a10");
        start(m);
        List<Object> tracks = listAt(app.ask(obj("id", 1, "cmd", "project")), "result", "tracks");
        assertEquals("a3f0c1d2-2b7e-4c55-9b1e-6d0f2c8e4a10", at(tracks, 1, "id"));
        assertFalse(((Map<?, ?>) tracks.get(0)).containsKey("id"), "no ID where Bitwig hasn't given one");
        assertEquals(List.of("index", "id", "name"), List.copyOf(((Map<?, ?>) tracks.get(1)).keySet()).subList(0, 3));
    }

    @Test
    @DisplayName("says what Bitwig knows of the remote controls: their steps, step names, origin and automation")
    void remoteControlsCarryTheirDetails() {
        start(LinkTest.song());
        bitwig.cursorDevice.show(device("Polysynth").page("Oscillator", "CONTENTS/PID1", "CONTENTS/PID2"),
                                 knob("CONTENTS/PID1", "Waveform", 1, "Square").steps("Sine", "Triangle", "Saw", "Square"),
                                 knob("CONTENTS/PID2", "Pan", 0.5, "C").origin(0.5).automated(),
                                 knob("CONTENTS/PID3", "Cutoff", 0.7, "2.0 kHz"));
        app.send(obj("id", 1, "cmd", "device"));
        bitwig.clock.advance(100);
        Object result = app.take().get(0).get("result");

        // A parameter that is one of the controls carries what Bitwig says of the control; the others are as before.
        assertJson(list(obj("id", "CONTENTS/PID1", "name", "Waveform", "value", 1, "display", "Square",
                            "steps", 4, "stepNames", list("Sine", "Triangle", "Saw", "Square"), "origin", 0, "automated", false),
                        obj("id", "CONTENTS/PID2", "name", "Pan", "value", 0.5, "display", "C", "steps", -1, "origin", 0.5, "automated", true),
                        obj("id", "CONTENTS/PID3", "name", "Cutoff", "value", 0.7, "display", "2.0 kHz")),
                   at(result, "params"));
        assertEquals("Oscillator", at(result, "page"));
        assertJson(list(obj("index", 0, "name", "Waveform", "value", 1, "display", "Square",
                            "steps", 4, "stepNames", list("Sine", "Triangle", "Saw", "Square"), "origin", 0, "automated", false,
                            "param", "CONTENTS/PID1"),
                        obj("index", 1, "name", "Pan", "value", 0.5, "display", "C", "steps", -1, "origin", 0.5, "automated", true,
                            "param", "CONTENTS/PID2")),
                   at(result, "controls"));
    }

    @Test
    @DisplayName("reads a plugin's knob from its remote control, and says a value is out of date, when Bitwig doesn't tell its new value")
    void pluginKnobsBitwigDoesntTell() {
        Model m = LinkTest.song();
        m.pluginValuesUntold = true;
        start(m);
        bitwig.cursorDevice.show(device("MO Probe Instrument").plugin().page("MO Probe", "CONTENTS/PID3e9"),
                                 knob("CONTENTS/PID3e9", "MO Knob A", 0.25, "0.250"),
                                 knob("CONTENTS/PID3ea", "MO Gain", 0.833, "0.0 dB"));
        assertJson(list(obj("id", "CONTENTS/PID3e9", "value", 0.25, "display", "0.250"), obj("id", "CONTENTS/PID3ea", "value", 0.833, "display", "0.0 dB")),
                   knobs(selected()));

        // The probe turns both: Bitwig's texts and its remote control follow, but it never tells the link the values.
        bitwig.cursorDevice.turnByPlugin("CONTENTS/PID3e9", 1, "1.000");
        bitwig.cursorDevice.turnByPlugin("CONTENTS/PID3ea", 0, "-60.0 dB");
        bitwig.clock.advance(50);
        List<Object> turned = knobs(selected());
        assertJson(list(obj("id", "CONTENTS/PID3e9", "value", 1, "display", "1.000"),
                        obj("id", "CONTENTS/PID3ea", "value", null, "display", "-60.0 dB", "stale", true)), turned);
        assertJson(list(obj("id", "CONTENTS/PID3e9", "value", 1, "display", "1.000"),
                        obj("id", "CONTENTS/PID3ea", "value", null, "display", "-60.0 dB", "stale", true)), knobs(selected()),
                   "still out of date, though its text hasn't changed since");

        // Once Bitwig tells the values again (selecting the device again, say), they're good.
        bitwig.cursorDevice.show(device("MO Probe Instrument").plugin().page("MO Probe", "CONTENTS/PID3e9"),
                                 knob("CONTENTS/PID3e9", "MO Knob A", 1, "1.000"),
                                 knob("CONTENTS/PID3ea", "MO Gain", 0, "-60.0 dB"));
        assertJson(list(obj("id", "CONTENTS/PID3e9", "value", 1, "display", "1.000"), obj("id", "CONTENTS/PID3ea", "value", 0, "display", "-60.0 dB")),
                   knobs(selected()));
    }

    @Test
    @DisplayName("doesn't call a value out of date when Bitwig has told it")
    void toldValuesArentStale() {
        start(LinkTest.song());
        bitwig.cursorDevice.show(device("Serum").plugin(), knob("CONTENTS/PID1", "Cutoff", 0.5, "50%"));
        knobs(selected());
        bitwig.cursorDevice.turnByPlugin("CONTENTS/PID1", 0.75, "75%");
        bitwig.clock.advance(50);
        assertJson(list(obj("id", "CONTENTS/PID1", "value", 0.75, "display", "75%")), knobs(selected()));
    }

    /** The selected device, as the link answers `device`. */
    Object selected() {
        app.send(obj("id", 1, "cmd", "device"));
        bitwig.clock.advance(100);
        return app.take().get(0).get("result");
    }

    /** Each parameter's ID, value and text, and whether its value is out of date. */
    static List<Object> knobs(Object device) {
        return listAt(device, "params").stream().map(p -> {
            Map<?, ?> m = (Map<?, ?>) p;
            Map<String, Object> k = new java.util.LinkedHashMap<>();
            for (String key : List.of("id", "value", "display", "stale"))
                if (m.containsKey(key)) k.put(key, m.get(key));
            return (Object) k;
        }).collect(Collectors.toList());
    }

    @Test
    @DisplayName("doesn't guess which parameter a remote control is when two look the same")
    void remoteControlsThatCouldBeEitherParameter() {
        start(LinkTest.song());
        bitwig.cursorDevice.show(device("Mixer").page("Levels", "CONTENTS/PID1"),
                                 knob("CONTENTS/PID1", "Level", 0.5, "0 dB"),
                                 knob("CONTENTS/PID2", "Level", 0.5, "0 dB"));
        app.send(obj("id", 1, "cmd", "device"));
        bitwig.clock.advance(100);
        Object result = app.take().get(0).get("result");
        for (Object p : listAt(result, "params")) assertFalse(((Map<?, ?>) p).containsKey("steps"), Json.write(p));
        assertEquals(null, at(result, "controls", 0, "param"));
        assertJson(1, ((List<?>) at(result, "controls")).size());
    }

    @Test
    @DisplayName("lists no remote controls for a device without a page, nor for none")
    void noRemoteControls() {
        start(LinkTest.song());
        bitwig.cursorDevice.show(device("Utility"), knob("a", "Gain", 0.5, "0 dB"));
        app.send(obj("id", 1, "cmd", "device"));
        bitwig.clock.advance(100);
        Object result = app.take().get(0).get("result");
        assertJson(obj("page", "", "controls", list()), obj("page", at(result, "page"), "controls", at(result, "controls")));
        assertJson(obj("id", "a", "name", "Gain", "value", 0.5, "display", "0 dB"), at(result, "params", 0));
    }

    @Test
    @DisplayName("says which parameter was clicked last, once Bitwig has said, without needing changes allowed")
    void saysWhichParameterWasClickedLast() {
        start(LinkTest.song());
        assertJson(obj("exists", false, "locked", false), app.ask(obj("id", 1, "cmd", "clicked")).get("result"));

        bitwig.click(knob("CONTENTS/PID9", "Filter Type", 0.5, "Band-pass").steps("Low-pass", "Band-pass", "High-pass"));
        assertEquals(false, at(app.ask(obj("id", 2, "cmd", "clicked")), "result", "exists"), "Bitwig says so a moment later");
        bitwig.clock.advance(50);
        assertJson(obj("exists", true, "name", "Filter Type", "value", 0.5, "display", "Band-pass", "steps", 3,
                       "stepNames", list("Low-pass", "Band-pass", "High-pass"), "origin", 0, "automated", false, "locked", false),
                   app.ask(obj("id", 3, "cmd", "clicked")).get("result"));

        // A continuous parameter has no step names.
        bitwig.click(knob("CONTENTS/PID3e8", "Cutoff", 0.25, "440 Hz").automated());
        bitwig.lastClicked.locked.v = true;
        bitwig.clock.advance(50);
        assertJson(obj("exists", true, "name", "Cutoff", "value", 0.25, "display", "440 Hz", "steps", -1, "origin", 0, "automated", true,
                       "locked", true),
                   app.ask(obj("id", 4, "cmd", "clicked")).get("result"));
    }

    @Test
    @DisplayName("starts and stops Bitwig's master recorder once changes are allowed, and says for how long it has recorded")
    void usesTheMasterRecorder() {
        start(LinkTest.song());
        for (Object what : new Object[] {"start", "stop", "status", null})
            assertMatches(LinkTest.CHANGES_OFF, error(app.ask(obj("id", 1, "cmd", "recorder", "do", what))));
        assertEquals(List.of(), bitwig.calls);

        bitwig.allowChanges(true);
        assertJson(obj("active", false, "duration", 0), app.ask(obj("id", 2, "cmd", "recorder")).get("result"));
        assertJson(obj("active", false, "duration", 0), app.ask(obj("id", 3, "cmd", "recorder", "do", "status")).get("result"));

        // It answers once Bitwig says it's recording: the stand-in starts 20 ms after it's asked, and the link looks every 50.
        app.send(obj("id", 4, "cmd", "recorder", "do", "start"));
        assertEquals(List.of(), app.take());
        bitwig.clock.advance(50);
        assertJson(obj("id", 4, "ok", true, "result", obj("active", true, "duration", 30)), app.take().get(0));
        bitwig.clock.advance(950);
        assertJson(obj("active", true, "duration", 980), app.ask(obj("id", 5, "cmd", "recorder")).get("result"));
        // Starting again asks nothing more of Bitwig (and answers when the link next looks).
        assertJson(obj("active", true, "duration", 1030), app.waitFor(obj("id", 6, "cmd", "recorder", "do", "start"), 50).get("result"));

        // Stopped 20 ms after it's asked: 1050 ms recorded in all.
        assertJson(obj("active", false, "duration", 1050), app.waitFor(obj("id", 7, "cmd", "recorder", "do", "stop")).get("result"));
        assertEquals(List.of(List.of("startRecording"), List.of("stopRecording")), bitwig.calls);
        assertEquals("Tell the recorder to \"start\", \"stop\" or give its \"status\", not rewind.",
                     error(app.ask(obj("id", 8, "cmd", "recorder", "do", "rewind"))));
    }

    @Test
    @DisplayName("starts the recorder only in the project the request names, and not once another comes to the front")
    void usesTheRecorderOnlyInTheProjectNamed() {
        start(LinkTest.song());
        bitwig.allowChanges(true);
        assertEquals(LinkTest.elsewhere("Night Drive", "MO Live Test"),
                     error(app.ask(obj("id", 1, "cmd", "recorder", "do", "start", "onlyIn", "MO Live Test"))));
        assertEquals(List.of(), bitwig.calls);

        // Asked while Night Drive is open, but another project comes to the front before it starts (behind a new track).
        app.send(obj("id", 2, "cmd", "newtrack", "name", "Lead", "onlyIn", "Night Drive"));
        app.send(obj("id", 3, "cmd", "recorder", "do", "start", "onlyIn", "Night Drive"));
        bitwig.clock.advance(10);
        bitwig.projectName.v = "My Song";
        bitwig.clock.advance(10000);
        assertEquals(List.of(LinkTest.elsewhere("My Song", "Night Drive"), LinkTest.elsewhere("My Song", "Night Drive")),
                     app.take().stream().map(Messages::error).collect(Collectors.toList()));
        assertEquals(List.of(List.of("createInstrumentTrack", -1)), bitwig.calls);

        bitwig.projectName.v = "Night Drive";
        assertJson(obj("active", true, "duration", 30),
                   app.waitFor(obj("id", 4, "cmd", "recorder", "do", "start", "onlyIn", "Night Drive"), 50).get("result"));
    }

    @Test
    @DisplayName("says when Bitwig doesn't start recording")
    void saysWhenRecordingDoesntStart() {
        Model m = LinkTest.song();
        m.engine = false;
        start(m);
        bitwig.allowChanges(true);
        app.send(obj("id", 1, "cmd", "recorder", "do", "start"));
        bitwig.clock.advance(4900);
        assertEquals(List.of(), app.take(), "waits for Bitwig");
        bitwig.clock.advance(200);
        assertEquals("Bitwig didn't start recording. Is its audio engine on?", error(app.take().get(0)));
        // The next change isn't held up.
        assertEquals(true, app.waitFor(obj("id", 2, "cmd", "newtrack", "name", "Lead")).get("ok"));
    }

    // ---- notes (extension 3.15) ----

    /** A note as notes answers with it. */
    static Map<String, Object> note(double start, double length, int key, int velocity) {
        return obj("start", start, "length", length, "key", key, "velocity", velocity);
    }

    @Test
    @DisplayName("reads the notes of the clip selected in the arranger, by start then key, without needing changes allowed")
    void readsTheArrangerClipsNotes() {
        start(LinkTest.song());
        // A C major chord, a bass note under it on channel 4, a D a bar on, and a note off the 16th-note grid, which reads
        // as starting on the 16th it's in. The notes go on past their first 16th, which doesn't make them start again.
        bitwig.state.arrangerNotes = new NoteClip("Bass", 0, 16).note(4, 2, 62, 0.8)
                                                                .note(0, 1.5, 67, 1)
                                                                .note(0, 1.5, 60, 100 / 127.0)
                                                                .note(0, 1.5, 64, 0.5)
                                                                .note(3, 0, 4, 36, 0.25)
                                                                .note(8.1, 0.5, 65, 0.6);
        bitwig.refresh();
        assertJson(obj("clips", list(obj("from", "arranger", "track", "Bass", "onSelectedTrack", true, "start", 0, "length", 16, "cut", false,
                                         "notes", list(note(0, 4, 36, 32), note(0, 1.5, 60, 100), note(0, 1.5, 64, 64), note(0, 1.5, 67, 127),
                                                       note(4, 2, 62, 102), note(8, 0.5, 65, 76))))),
                   app.ask(obj("id", 1, "cmd", "notes")).get("result"));
        assertEquals(List.of(), bitwig.calls);
    }

    @Test
    @DisplayName("reads the clips selected in the arranger and in the clip launcher, the arranger's first, and follows the selection")
    void readsBothClips() {
        start(LinkTest.song());
        bitwig.state.launcherNotes = new NoteClip("Keys", 2, 8).note(2, 1, 57, 0.5).note(2, 1, 60, 0.5);
        bitwig.state.arrangerNotes = new NoteClip("Bass", 0, 300).note(0, 4, 40, 1);
        bitwig.refresh();
        assertJson(obj("clips", list(obj("from", "arranger", "track", "Bass", "onSelectedTrack", true, "start", 0, "length", 300, "cut", true,
                                         "notes", list(note(0, 4, 40, 127))),
                                     obj("from", "launcher", "track", "Keys", "onSelectedTrack", false, "start", 2, "length", 8, "cut", false,
                                         "notes", list(note(2, 1, 57, 64), note(2, 1, 60, 64))))),
                   app.ask(obj("id", 1, "cmd", "notes")).get("result"));

        // Another launcher clip selected, and none in the arranger: only the new clip's notes.
        bitwig.state.launcherNotes = new NoteClip("Keys", 0, 4).note(1, 1, 62, 0.5);
        bitwig.state.arrangerNotes = null;
        bitwig.refresh();
        assertJson(obj("clips", list(obj("from", "launcher", "track", "Keys", "onSelectedTrack", false, "start", 0, "length", 4, "cut", false,
                                         "notes", list(note(1, 1, 62, 64))))),
                   app.ask(obj("id", 2, "cmd", "notes")).get("result"));
    }

    @Test
    @DisplayName("says how to select a clip when none is selected")
    void saysWhenNoClipIsSelected() {
        start(LinkTest.song());
        assertEquals("No clip is selected in Bitwig. Click your chord clip in the arranger or the clip launcher, then try again.",
                     error(app.ask(obj("id", 1, "cmd", "notes"))));

        // An empty clip is still a clip.
        bitwig.state.arrangerNotes = new NoteClip("Bass", 0, 4);
        bitwig.refresh();
        assertJson(obj("clips", list(obj("from", "arranger", "track", "Bass", "onSelectedTrack", true, "start", 0, "length", 4, "cut", false,
                                         "notes", list()))),
                   app.ask(obj("id", 2, "cmd", "notes")).get("result"));
    }

    @Test
    @DisplayName("says the self-check tells why when Bitwig wouldn't make the cursor clips notes reads")
    void notesWithoutTheCursorClips() {
        Model m = LinkTest.song();
        m.broken.add("createLauncherCursorClip");
        start(m);
        assertEquals(true, app.ask(obj("id", 1, "cmd", "project")).get("ok"));
        assertEquals("This Bitwig didn't let the link read its clips' notes. A selftest says why.", error(app.ask(obj("id", 2, "cmd", "notes"))));
        Map<String, Object> result = SelfCheckTest.map(app.ask(obj("id", 3, "cmd", "selftest")).get("result"));
        assertEquals(List.of("clip notes"), listAt(result, "checks").stream().filter(c -> !(Boolean) at(c, "ok")).map(c -> at(c, "name"))
                                                                    .collect(Collectors.toList()));
        assertMatches("^Couldn't set it up: java.lang.AbstractMethodError: ", SelfCheckTest.detail(result, "clip notes"));
    }

    // ---- following and playing in Bitwig (extension 3.16) ----

    /** The song with a clip on the bass track's first slot, and the bass track's ID. */
    void startWithABassClip() {
        Model m = LinkTest.song();
        m.scenes = 1;
        m.tracks.get(1).id("bass-id").slots("old.mid");
        start(m);
    }

    @Test
    @DisplayName("says whether Bitwig is playing, where and how fast, without needing changes allowed")
    void saysWhereBitwigIsPlaying() {
        startWithABassClip();
        assertJson(obj("playing", false, "position", 0, "playStart", 0, "tempo", 124, "timeSignature", "7/8"),
                   app.ask(obj("id", 1, "cmd", "transport")).get("result"));

        // Playing from the play start, Bitwig moves on at its tempo.
        bitwig.allowChanges(true);
        app.waitFor(obj("id", 2, "cmd", "clip", "trackId", "bass-id", "slot", 0, "path", "C:\\MIDI\\Bass.mid", "replace", true, "launch", true));
        Map<String, Object> playing = SelfCheckTest.map(app.ask(obj("id", 3, "cmd", "transport")).get("result"));
        assertEquals(true, playing.get("playing"));
        assertTrue((Double) playing.get("position") > 0, Json.write(playing));
    }

    @Test
    @DisplayName("swaps a slot's clip for a file and launches it, which starts Bitwig playing")
    void replacesAndLaunchesAClip() {
        startWithABassClip();
        bitwig.allowChanges(true);
        assertJson(obj("slot", 0, "launched", true),
                   app.waitFor(obj("id", 1, "cmd", "clip", "trackId", "bass-id", "slot", 0, "path", "C:\\MIDI\\Bass.mid", "replace", true, "launch", true))
                      .get("result"));
        assertEquals(List.of("C:\\MIDI\\Bass.mid"), bitwig.state.tracks.get(1).slots);
        assertTrue(bitwig.calls.contains(List.of("launch", 1, 0)), String.valueOf(bitwig.calls));
        // In step with the song, so a swapped clip carries on with the parts that didn't change (extension 3.17).
        assertTrue(bitwig.calls.contains(List.of("launchWithOptions", 1, 0, "default", "synced")), String.valueOf(bitwig.calls));
        assertTrue(bitwig.playingNow);

        // Swapped for the other file and launched again: launched once the new clip is there, not after a fixed wait.
        assertJson(obj("slot", 0, "launched", true),
                   app.waitFor(obj("id", 4, "cmd", "clip", "trackId", "bass-id", "slot", 0, "path", "C:\\MIDI\\Bass 3.mid", "replace", true, "launch", true))
                      .get("result"));
        assertEquals(List.of("C:\\MIDI\\Bass 3.mid"), bitwig.state.tracks.get(1).slots);
        assertEquals(2, bitwig.calls.stream().filter(c -> c.get(0).equals("launchWithOptions")).count(), String.valueOf(bitwig.calls));

        // Swapping it again without launching leaves it as it is; replacing needs the slot said.
        assertJson(obj("slot", 0), app.waitFor(obj("id", 2, "cmd", "clip", "trackId", "bass-id", "slot", 0, "path", "C:\\MIDI\\Bass 2.mid", "replace", true))
                                      .get("result"));
        assertEquals(List.of("C:\\MIDI\\Bass 2.mid"), bitwig.state.tracks.get(1).slots);
        assertEquals("Say which slot's clip to replace.",
                     error(app.ask(obj("id", 3, "cmd", "clip", "trackId", "bass-id", "path", "C:\\MIDI\\x.mid", "replace", true))));
    }

    @Test
    @DisplayName("waits as long for a replace into an empty slot as for any new clip, so a new track's first part isn't lost")
    void waitsForAReplaceIntoAnEmptySlotAsForANewClip() {
        // The Playground always replaces slot 0, even on a track it has just made, and Bitwig may take a while to read
        // the file: more than a swap's wait, less than a new clip's (extension 3.18).
        Model m = LinkTest.song();
        m.scenes = 1;
        m.tracks.get(1).id("bass-id");
        m.clipDelay = 3000;
        start(m);
        bitwig.allowChanges(true);
        assertJson(obj("slot", 0, "launched", true),
                   app.waitFor(obj("id", 1, "cmd", "clip", "trackId", "bass-id", "slot", 0, "path", "C:\\MIDI\\Bass.mid", "replace", true, "launch", true))
                      .get("result"));
        assertEquals(List.of("C:\\MIDI\\Bass.mid"), bitwig.state.tracks.get(1).slots);
        assertTrue(bitwig.calls.contains(List.of("launchWithOptions", 1, 0, "default", "synced")), String.valueOf(bitwig.calls));
    }

    @Test
    @DisplayName("stops the launcher clips on the tracks given and puts them back on the arranger, leaving out tracks that are gone")
    void stopsTracks() {
        startWithABassClip();
        bitwig.allowChanges(true);
        app.waitFor(obj("id", 1, "cmd", "clip", "trackId", "bass-id", "slot", 0, "path", "C:\\MIDI\\Bass.mid", "replace", true, "launch", true));
        assertJson(obj("stopped", list("bass-id")),
                   app.waitFor(obj("id", 2, "cmd", "stoptracks", "trackIds", list("bass-id", "gone-id"))).get("result"));
        assertTrue(bitwig.calls.contains(List.of("stopTrack", 1)), String.valueOf(bitwig.calls));
        assertTrue(bitwig.calls.contains(List.of("returnToArrangement", 1)), String.valueOf(bitwig.calls));
        assertEquals("Say which tracks to stop, by their IDs.", error(app.waitFor(obj("id", 3, "cmd", "stoptracks"))));
    }

    @Test
    @DisplayName("still stops the tracks once the app that asked has closed, after the clip waiting in front is launched")
    void stopsTracksAfterTheAppHasClosed() {
        // The Playground sends stoptracks as it closes and doesn't wait for the clip it sent just before.
        startWithABassClip();
        bitwig.model.clipDelay = 1000;
        bitwig.allowChanges(true);
        app.send(obj("id", 1, "cmd", "clip", "trackId", "bass-id", "slot", 0, "path", "C:\\MIDI\\Bass.mid", "replace", true, "launch", true));
        app.send(obj("id", 2, "cmd", "stoptracks", "trackIds", list("bass-id")));
        app.close();
        bitwig.clock.advance(10000);
        int launched = bitwig.calls.indexOf(List.of("launchWithOptions", 1, 0, "default", "synced"));
        int stopped = bitwig.calls.indexOf(List.of("stopTrack", 1));
        assertTrue(launched >= 0 && stopped > launched, String.valueOf(bitwig.calls));
    }

    // ---- playing a part on the selected track (extension 3.19) ----

    @Test
    @DisplayName("says which track is selected, without needing changes allowed")
    void saysWhichTrackIsSelected() {
        startWithABassClip();
        assertJson(obj("index", 1, "id", "bass-id", "name", "Bass", "type", "Instrument"),
                   app.ask(obj("id", 1, "cmd", "selectedtrack")).get("result"));
    }

    @Test
    @DisplayName("says no track is selected when none is")
    void saysNoTrackIsSelected() {
        Model m = LinkTest.song();
        m.selectedTrack = null;
        start(m);
        Map<String, Object> result = SelfCheckTest.map(app.ask(obj("id", 1, "cmd", "selectedtrack")).get("result"));
        assertTrue(result.containsKey("index") && result.get("index") == null, Json.write(result));
    }

    @Test
    @DisplayName("takes out an audition clip it put in, and stops Bitwig when asked")
    void deletesAnAuditionClip() {
        Model m = LinkTest.song();
        m.scenes = 2;
        m.tracks.get(1).id("bass-id").slots("old.mid");
        start(m);
        bitwig.allowChanges(true);
        assertJson(obj("slot", 1, "launched", true),
                   app.waitFor(obj("id", 1, "cmd", "clip", "trackId", "bass-id", "path", "C:\\MIDI\\MO Play 1.mid", "launch", true,
                                   "audition", true)).get("result"));
        assertJson(obj("deleted", true),
                   app.waitFor(obj("id", 2, "cmd", "deleteclip", "trackId", "bass-id", "slot", 1, "stop", true, "transport", true))
                      .get("result"));
        assertTrue(bitwig.calls.contains(List.of("deleteClip", 1, 1)), String.valueOf(bitwig.calls));
        assertTrue(bitwig.calls.contains(List.of("stopTrack", 1)), String.valueOf(bitwig.calls));
        assertTrue(bitwig.calls.contains(List.of("returnToArrangement", 1)), String.valueOf(bitwig.calls));
        assertTrue(bitwig.calls.contains(List.of("stop")), String.valueOf(bitwig.calls));
        // Your own clip stays where it is.
        assertEquals("old.mid", bitwig.state.tracks.get(1).slots.get(0));
        // Only once: a second ask finds nothing of the app's there.
        assertJson(obj("deleted", false), app.waitFor(obj("id", 3, "cmd", "deleteclip", "trackId", "bass-id", "slot", 1)).get("result"));
    }

    @Test
    @DisplayName("never takes out a clip it didn't put in for an audition")
    void leavesOtherClipsAlone() {
        startWithABassClip();
        bitwig.allowChanges(true);
        assertJson(obj("deleted", false), app.waitFor(obj("id", 1, "cmd", "deleteclip", "trackId", "bass-id", "slot", 0)).get("result"));
        // A clip sent without "audition" isn't the app's to take out either.
        app.waitFor(obj("id", 2, "cmd", "clip", "trackId", "bass-id", "slot", 0, "path", "C:\\MIDI\\Bass.mid", "replace", true));
        assertJson(obj("deleted", false), app.waitFor(obj("id", 3, "cmd", "deleteclip", "trackId", "bass-id", "slot", 0)).get("result"));
        assertFalse(bitwig.calls.stream().anyMatch(c -> c.get(0).equals("deleteClip")), String.valueOf(bitwig.calls));
        assertEquals("Say which track, by its ID.", error(app.waitFor(obj("id", 4, "cmd", "deleteclip", "slot", 0))));
    }

    // ---- playing a part with its song's bars going round (extension 3.22) ----

    /** A part to play with bars 9 to 16 of a song in 4/4: from beat 32, for 32 beats. */
    static Map<String, Object> partWithBars(int id, String file) {
        return obj("id", id, "cmd", "clip", "trackId", "bass-id", "path", "C:\\MIDI\\" + file, "launch", true, "audition", true,
                   "loop", obj("start", 32, "length", 32));
    }

    static Map<String, Object> takeOut(int id, int slot) {
        return obj("id", id, "cmd", "deleteclip", "trackId", "bass-id", "slot", slot, "stop", true, "transport", true);
    }

    /** The song in 4/4, with a loop of your own (off, 2 bars from bar 17) and Play starting at bar 3. */
    void startWithALoopOfYourOwn() {
        Model m = LinkTest.song();
        m.scenes = 3;
        m.timeSignature = "4/4";
        m.tracks.get(1).id("bass-id").slots("old.mid");
        m.transport.put("isArrangerLoopEnabled", false);
        m.loopStart = 64;
        m.loopLength = 8;
        m.playStart = 8;
        start(m);
        bitwig.allowChanges(true);
    }

    /** The arranger's loop and the play start, as Bitwig has them now. */
    List<Object> loopNow() {
        return List.of(bitwig.transportNow.get("isArrangerLoopEnabled"), bitwig.loopStartAt, bitwig.loopLengthAt, bitwig.playStartAt);
    }

    @Test
    @DisplayName("loops the song's bars for a part, starts Bitwig at the first of them, and puts the loop back when the part comes out")
    void loopsTheSongsBarsForAPart() {
        startWithALoopOfYourOwn();
        assertJson(obj("slot", 1, "launched", true, "looping", true), app.waitFor(partWithBars(1, "MO Play 1.mid")).get("result"));
        assertEquals(List.of(true, 32.0, 32.0, 32.0), loopNow());
        // At once and from its own start, so the part's first bar is the bars' first.
        assertTrue(bitwig.calls.contains(List.of("launchWithOptions", 1, 1, "none", "from_start")), String.valueOf(bitwig.calls));
        // Bitwig wasn't playing, so it isn't stopped (Stop would move your play start); it plays from the bars' start.
        assertFalse(bitwig.calls.contains(List.of("stop")), String.valueOf(bitwig.calls));
        assertTrue(bitwig.playingNow);
        assertEquals(32.0, bitwig.state.tracks.get(1).launched.start);

        assertJson(obj("deleted", true, "loopBack", true), app.waitFor(takeOut(2, 1)).get("result"));
        assertFalse(bitwig.playingNow);
        assertEquals(List.of(false, 64.0, 8.0, 8.0), loopNow());
        assertEquals("old.mid", bitwig.state.tracks.get(1).slots.get(0));
        // Only once: nothing of the app's is left there, and nothing more is put back.
        assertJson(obj("deleted", false), app.waitFor(obj("id", 3, "cmd", "deleteclip", "trackId", "bass-id", "slot", 1)).get("result"));
    }

    @Test
    @DisplayName("stops Bitwig first when it plays, so it starts again at the song's bars")
    void stopsBitwigBeforeLoopingTheBars() {
        startWithALoopOfYourOwn();
        app.waitFor(obj("id", 1, "cmd", "clip", "trackId", "bass-id", "slot", 0, "path", "C:\\MIDI\\Bass.mid", "replace", true, "launch", true));
        assertTrue(bitwig.playingNow);
        assertJson(obj("slot", 1, "launched", true, "looping", true), app.waitFor(partWithBars(2, "MO Play 1.mid")).get("result"));
        int stopped = bitwig.calls.indexOf(List.of("stop"));
        int looped = bitwig.calls.indexOf(List.of("loopStart", 32.0));
        int launched = bitwig.calls.indexOf(List.of("launchWithOptions", 1, 1, "none", "from_start"));
        assertTrue(stopped >= 0 && looped > stopped && launched > looped, String.valueOf(bitwig.calls));
        assertTrue(bitwig.playingNow);
        assertEquals(32.0, bitwig.state.tracks.get(1).launched.start);
        assertJson(obj("deleted", true, "loopBack", true), app.waitFor(takeOut(3, 1)).get("result"));
        assertEquals(List.of(false, 64.0, 8.0, 8.0), loopNow());
    }

    @Test
    @DisplayName("leaves what you changed while the part played, and puts the rest back even when the clip is yours now")
    void leavesWhatYouChangedMeanwhile() {
        startWithALoopOfYourOwn();
        app.waitFor(partWithBars(1, "MO Play 1.mid"));
        // You drag the loop's end out and click in bar 5, and rename the clip to keep it.
        bitwig.loopLengthAt = 48;
        bitwig.playStartAt = 16;
        bitwig.state.tracks.get(1).slots.set(1, "Kept it.mid");
        bitwig.refresh();
        assertJson(obj("deleted", false, "loopBack", true), app.waitFor(takeOut(2, 1)).get("result"));
        assertEquals(List.of(false, 64.0, 48.0, 16.0), loopNow());
        assertEquals("Kept it.mid", bitwig.state.tracks.get(1).slots.get(1));
    }

    @Test
    @DisplayName("doesn't press Stop on a Bitwig you stopped yourself, which would move your play start to the beginning")
    void doesntStopABitwigAlreadyStopped() {
        startWithALoopOfYourOwn();
        app.waitFor(partWithBars(1, "MO Play 1.mid"));
        bitwig.pressStop();
        assertJson(obj("deleted", true, "loopBack", true), app.waitFor(takeOut(2, 1)).get("result"));
        assertFalse(bitwig.calls.contains(List.of("stop")), String.valueOf(bitwig.calls));
        assertFalse(bitwig.calls.contains(List.of("stopWhileStopped")), String.valueOf(bitwig.calls));
        assertEquals(List.of(false, 64.0, 8.0, 8.0), loopNow());
    }

    @Test
    @DisplayName("waits for a part taken out straight after its launch to start Bitwig, and stops it then: never a Bitwig still stopped")
    void stopsAPartTakenOutAsItStarts() {
        startWithALoopOfYourOwn();
        bitwig.model.startDelay = 400;
        // The clip answers once it's launched; Bitwig starts 400 ms later, after the app has asked to take it out.
        app.send(partWithBars(1, "MO Play 1.mid"));
        app.send(takeOut(2, 1));
        bitwig.clock.advance(10000);
        List<Map<String, Object>> replies = app.take();
        assertJson(obj("slot", 1, "launched", true, "looping", true), replies.get(0).get("result"));
        assertJson(obj("deleted", true, "loopBack", true), replies.get(1).get("result"));
        assertTrue(bitwig.calls.contains(List.of("stop")), String.valueOf(bitwig.calls));
        // Stop on a Bitwig still stopped would have moved your play start to the beginning, and not come back.
        assertFalse(bitwig.calls.contains(List.of("stopWhileStopped")), String.valueOf(bitwig.calls));
        assertFalse(bitwig.playingNow);
        assertEquals(List.of(false, 64.0, 8.0, 8.0), loopNow());
    }

    @Test
    @DisplayName("takes a part out without a long wait when its launch never starts Bitwig")
    void takesOutAPartThatNeverStarted() {
        startWithALoopOfYourOwn();
        // Bitwig would only start an hour after the launch: it never does while the part is in.
        bitwig.model.startDelay = 3600000;
        app.send(partWithBars(1, "MO Play 1.mid"));
        app.send(takeOut(2, 1));
        bitwig.clock.advance(10000);
        List<Map<String, Object>> replies = app.take();
        assertJson(obj("slot", 1, "launched", true, "looping", true), replies.get(0).get("result"));
        assertJson(obj("deleted", true, "loopBack", true), replies.get(1).get("result"));
        // Bitwig was never seen playing, so it's never told to stop.
        assertFalse(bitwig.calls.contains(List.of("stop")), String.valueOf(bitwig.calls));
        assertEquals(List.of(false, 64.0, 8.0, 8.0), loopNow());
    }

    @Test
    @DisplayName("puts the loop back after the newest part, though one before it was never taken out")
    void aPartNeverTakenOutDoesntKeepTheLoop() {
        startWithALoopOfYourOwn();
        // The app let go of the first (it couldn't stop it), and plays a second with other bars.
        app.waitFor(partWithBars(1, "MO Play 1.mid"));
        Map<String, Object> second = partWithBars(2, "MO Play 2.mid");
        second.put("loop", obj("start", 64, "length", 16));
        assertJson(obj("slot", 2, "launched", true, "looping", true), app.waitFor(second).get("result"));
        assertEquals(List.of(true, 64.0, 16.0, 64.0), loopNow());
        // The second comes out: what goes back is what was found before the first. (The loop's start is where yours was.)
        assertJson(obj("deleted", true, "loopBack", true), app.waitFor(takeOut(3, 2)).get("result"));
        assertEquals(List.of(false, 64.0, 8.0, 8.0), loopNow());
        // The first can still come out; there's no loop left to put back.
        assertJson(obj("deleted", true), app.waitFor(obj("id", 4, "cmd", "deleteclip", "trackId", "bass-id", "slot", 1)).get("result"));
        assertEquals(List.of(false, 64.0, 8.0, 8.0), loopNow());
    }

    @Test
    @DisplayName("takes what you changed after a part that was never taken out as yours, when the next part plays")
    void whatYouChangedAfterAPartNeverTakenOutIsYours() {
        startWithALoopOfYourOwn();
        app.waitFor(partWithBars(1, "MO Play 1.mid"));
        // The app let go of it. You stop Bitwig, drag the loop's end out and click in bar 5.
        bitwig.pressStop();
        bitwig.loopLengthAt = 48;
        bitwig.playStartAt = 16;
        bitwig.refresh();
        Map<String, Object> second = partWithBars(2, "MO Play 2.mid");
        second.put("loop", obj("start", 64, "length", 16));
        app.waitFor(second);
        assertEquals(List.of(true, 64.0, 16.0, 64.0), loopNow());
        // Your length and play start go back, not the ones from before the first part; the loop goes off as it was then.
        assertJson(obj("deleted", true, "loopBack", true), app.waitFor(takeOut(3, 2)).get("result"));
        assertEquals(List.of(false, 64.0, 48.0, 16.0), loopNow());
    }

    @Test
    @DisplayName("still puts the loop back at the next try, after a take-out that was stopped part way")
    void putsTheLoopBackAfterATakeOutStoppedPartWay() {
        startWithALoopOfYourOwn();
        app.waitFor(partWithBars(1, "MO Play 1.mid"));
        // Changes go off just after the take-out has begun: it stops with the loop still on the song's bars.
        app.send(takeOut(2, 1));
        bitwig.clock.advance(10);
        bitwig.allowChanges(false);
        bitwig.clock.advance(10000);
        assertMatches(LinkTest.CHANGES_OFF, error(app.take().get(0)));
        assertEquals(List.of(true, 32.0, 32.0, 32.0), loopNow());
        // Stop pressed again in the app, with changes back on: the clip went the first time, and the loop goes back now.
        bitwig.allowChanges(true);
        assertJson(obj("deleted", false, "loopBack", true), app.waitFor(takeOut(3, 1)).get("result"));
        assertEquals(List.of(false, 64.0, 8.0, 8.0), loopNow());
    }

    @Test
    @DisplayName("says so when it had stopped a Bitwig that was playing for a part that then didn't go in")
    void saysItStoppedBitwigForAPartThatDidntGoIn() {
        startWithALoopOfYourOwn();
        app.waitFor(obj("id", 1, "cmd", "clip", "trackId", "bass-id", "slot", 0, "path", "C:\\MIDI\\Bass.mid", "replace", true, "launch", true));
        assertTrue(bitwig.playingNow);
        assertEquals("Bitwig didn't make a clip from C:\\MIDI\\notes.txt. Bitwig was playing: the link had stopped it, to start it at the song's bars.",
                     error(app.waitFor(partWithBars(2, "notes.txt"))));
        bitwig.clock.advance(1000);
        assertEquals(List.of(false, 64.0, 8.0, 8.0), loopNow());
        assertFalse(bitwig.playingNow);
    }

    @Test
    @DisplayName("says in hello when this Bitwig doesn't let it reach the arranger's loop, and refuses bars to loop then")
    void saysWhenItCantReachTheLoop() {
        Model m = LinkTest.song();
        m.timeSignature = "4/4";
        m.tracks.get(1).id("bass-id");
        m.broken.add("arrangerLoopStart");
        start(m);
        bitwig.allowChanges(true);
        assertEquals(false, at(app.ask(obj("id", 1, "cmd", "hello")), "result", "songLoop"));
        assertEquals("This Bitwig didn't let the link set the arranger's loop. A selftest says why.",
                     error(app.waitFor(partWithBars(2, "MO Play 1.mid"))));
        // The part alone still plays there.
        assertJson(obj("slot", 0, "launched", true),
                   app.waitFor(obj("id", 3, "cmd", "clip", "trackId", "bass-id", "path", "C:\\MIDI\\MO Play 1.mid", "launch", true,
                                   "audition", true)).get("result"));
    }

    @Test
    @DisplayName("puts the loop back when Bitwig makes no clip from the part's file")
    void putsTheLoopBackWhenThePartDoesntGoIn() {
        startWithALoopOfYourOwn();
        // The stand-in makes clips from MIDI files only, as Bitwig makes none from a file it can't read.
        assertEquals("Bitwig didn't make a clip from C:\\MIDI\\notes.txt.", error(app.waitFor(partWithBars(1, "notes.txt"))));
        bitwig.clock.advance(1000);
        assertEquals(List.of(false, 64.0, 8.0, 8.0), loopNow());
        assertFalse(bitwig.playingNow);
    }

    @Test
    @DisplayName("says when Bitwig doesn't set its loop, and leaves it as it was")
    void saysWhenBitwigDoesntSetItsLoop() {
        Model m = LinkTest.song();
        m.timeSignature = "4/4";
        m.tracks.get(1).id("bass-id");
        m.transport.put("isArrangerLoopEnabled", false);
        m.settingsIgnored.add("arrangerLoopDuration");
        start(m);
        bitwig.allowChanges(true);
        assertEquals("Bitwig didn't set its loop to the song's bars (loop length).", error(app.waitFor(partWithBars(1, "MO Play 1.mid"))));
        bitwig.clock.advance(1000);
        assertEquals(List.of(false, 0.0, 16.0, 0.0), loopNow());
        assertEquals(List.of(), bitwig.state.tracks.get(1).slots.stream().filter(c -> c != null).collect(Collectors.toList()));
    }

    @Test
    @DisplayName("only loops bars for a part it plays and takes out again, and only bars that are bars")
    void refusesALoopItCouldntPutBack() {
        startWithALoopOfYourOwn();
        String only = "Only a part the app plays and takes out again (launch and audition) can loop the song's bars.";
        Map<String, Object> kept = partWithBars(1, "MO Play 1.mid");
        kept.remove("audition");
        assertEquals(only, error(app.waitFor(kept)));
        Map<String, Object> silent = partWithBars(2, "MO Play 1.mid");
        silent.remove("launch");
        assertEquals(only, error(app.waitFor(silent)));
        Map<String, Object> backwards = partWithBars(3, "MO Play 1.mid");
        backwards.put("loop", obj("start", -4, "length", 16));
        assertEquals("The loop's start is a number of beats, 0 or more, not -4.", error(app.waitFor(backwards)));
        Map<String, Object> empty = partWithBars(4, "MO Play 1.mid");
        empty.put("loop", obj("start", 0, "length", 0));
        assertEquals("The loop's length is a number of beats, more than 0, not 0.", error(app.waitFor(empty)));
        Map<String, Object> words = partWithBars(5, "MO Play 1.mid");
        words.put("loop", "bars 9 to 16");
        assertEquals("The bars to loop are a start and a length in beats, not bars 9 to 16.", error(app.waitFor(words)));
        // Nothing changed for any of them.
        assertEquals(List.of(false, 64.0, 8.0, 8.0), loopNow());
        assertEquals(Arrays.asList("old.mid"), bitwig.state.tracks.get(1).slots.stream().filter(c -> c != null).collect(Collectors.toList()));
        assertFalse(bitwig.playingNow);
    }

    @Test
    @DisplayName("puts the loop back when Bitwig stops the link while a part plays with its song's bars")
    void putsTheLoopBackWhenTheLinkStops() {
        startWithALoopOfYourOwn();
        app.waitFor(partWithBars(1, "MO Play 1.mid"));
        bitwig.link.exit();
        bitwig.clock.advance(1000);
        assertEquals(List.of(false, 64.0, 8.0, 8.0), loopNow());
    }

    // ---- marking a bar with the play start (extension 3.23) ----

    @Test
    @DisplayName("moves the play start to a bar for a part sent for a song, and leaves a Bitwig that's playing to play on")
    void marksABarWithThePlayStart() {
        startWithALoopOfYourOwn();
        assertJson(obj("was", 8, "playStart", 32), app.waitFor(obj("id", 1, "cmd", "playstart", "at", 32)).get("result"));
        assertEquals(List.of(false, 64.0, 8.0, 32.0), loopNow());
        assertFalse(bitwig.playingNow);
        // Asked again for where it is already: nothing is set, and it says so all the same.
        int sets = (int) bitwig.calls.stream().filter(c -> c.get(0).equals("playStart")).count();
        assertJson(obj("was", 32, "playStart", 32), app.waitFor(obj("id", 2, "cmd", "playstart", "at", 32)).get("result"));
        assertEquals(sets, (int) bitwig.calls.stream().filter(c -> c.get(0).equals("playStart")).count());
        // While Bitwig plays, it plays on: the play start is where the next Play begins.
        app.waitFor(obj("id", 3, "cmd", "clip", "trackId", "bass-id", "slot", 0, "path", "C:\\MIDI\\Bass.mid", "replace", true, "launch", true));
        assertTrue(bitwig.playingNow);
        assertJson(obj("was", 32, "playStart", 64), app.waitFor(obj("id", 4, "cmd", "playstart", "at", 64)).get("result"));
        assertTrue(bitwig.playingNow);
        assertFalse(bitwig.calls.contains(List.of("stop")), String.valueOf(bitwig.calls));
    }

    @Test
    @DisplayName("keeps a mark asked for while a part plays with its song's bars: the play start goes back to the mark")
    void aMarkOutlastsAPartPlayingWithItsBars() {
        startWithALoopOfYourOwn();
        app.waitFor(partWithBars(1, "MO Play 1.mid"));
        assertEquals(List.of(true, 32.0, 32.0, 32.0), loopNow());
        // The part sent is for the same bars, so the play start is there already: it's the mark's from now on.
        assertJson(obj("was", 32, "playStart", 32), app.waitFor(obj("id", 2, "cmd", "playstart", "at", 32)).get("result"));
        assertJson(obj("deleted", true, "loopBack", true), app.waitFor(takeOut(3, 1)).get("result"));
        assertEquals(List.of(false, 64.0, 8.0, 32.0), loopNow());
        // A mark somewhere else than the bars going round stays too.
        app.waitFor(partWithBars(4, "MO Play 2.mid"));
        assertJson(obj("was", 32, "playStart", 96), app.waitFor(obj("id", 5, "cmd", "playstart", "at", 96)).get("result"));
        assertJson(obj("deleted", true, "loopBack", true), app.waitFor(takeOut(6, 1)).get("result"));
        assertEquals(List.of(false, 64.0, 8.0, 96.0), loopNow());
    }

    @Test
    @DisplayName("only marks a bar with changes on, in the project asked for, and says when Bitwig doesn't move the play start")
    void refusesAMarkItMayNotMake() {
        Model m = LinkTest.song();
        m.timeSignature = "4/4";
        m.playStart = 8;
        m.playStartIgnored = true;
        start(m);
        assertMatches(LinkTest.CHANGES_OFF, error(app.waitFor(obj("id", 1, "cmd", "playstart", "at", 32))));
        bitwig.allowChanges(true);
        assertEquals("Bitwig has \"Night Drive\" open, not \"Glass\", so the link stopped before changing anything there.",
                     error(app.waitFor(obj("id", 2, "cmd", "playstart", "at", 32, "onlyIn", "Glass"))));
        assertEquals("at is a number of beats, 0 or more, not -4.", error(app.waitFor(obj("id", 3, "cmd", "playstart", "at", -4))));
        assertEquals("at is a number of beats, 0 or more, not undefined.", error(app.waitFor(obj("id", 4, "cmd", "playstart"))));
        assertFalse(bitwig.calls.stream().anyMatch(c -> c.get(0).equals("playStart")), String.valueOf(bitwig.calls));
        // This Bitwig takes the ask and doesn't move it.
        assertEquals("Bitwig didn't move the play start to beat 32.", error(app.waitFor(obj("id", 5, "cmd", "playstart", "at", 32))));
        assertEquals(8.0, bitwig.playStartAt);
    }

    @Test
    @DisplayName("says when this Bitwig doesn't let it reach the play start")
    void saysWhenItCantReachThePlayStart() {
        Model m = LinkTest.song();
        m.broken.add("playStartPosition");
        start(m);
        bitwig.allowChanges(true);
        assertEquals("This Bitwig didn't let the link reach the play start. A selftest says why.",
                     error(app.waitFor(obj("id", 1, "cmd", "playstart", "at", 32))));
    }

    // ---- auditioning a preset on a track of its own (extension 3.20) ----

    @Test
    @DisplayName("takes out a track it made for an audition, and stops Bitwig when asked")
    void deletesAnAuditionTrack() {
        Model m = LinkTest.song();
        m.newTrackIds = true;
        start(m);
        bitwig.allowChanges(true);
        int before = bitwig.state.tracks.size();
        Map<String, Object> made = SelfCheckTest.map(
            app.waitFor(obj("id", 1, "cmd", "newtrack", "name", "Warm Pad", "audition", true)).get("result"));
        String id = (String) made.get("id");
        assertEquals(before + 1, bitwig.state.tracks.size());
        assertJson(obj("deleted", true),
                   app.waitFor(obj("id", 2, "cmd", "deletetrack", "trackId", id, "transport", true)).get("result"));
        assertEquals(before, bitwig.state.tracks.size());
        assertTrue(bitwig.calls.contains(List.of("stop")), String.valueOf(bitwig.calls));
        // Only once: a second ask finds nothing of the app's there.
        assertJson(obj("deleted", false), app.waitFor(obj("id", 3, "cmd", "deletetrack", "trackId", id)).get("result"));
    }

    @Test
    @DisplayName("never takes out a track it didn't make for an audition, or one renamed since")
    void leavesOtherTracksAlone() {
        Model m = LinkTest.song();
        m.newTrackIds = true;
        m.tracks.get(1).id("bass-id");
        start(m);
        bitwig.allowChanges(true);
        int before = bitwig.state.tracks.size();
        assertJson(obj("deleted", false), app.waitFor(obj("id", 1, "cmd", "deletetrack", "trackId", "bass-id")).get("result"));
        // A track made without "audition" isn't the app's to take out.
        String plain = (String) SelfCheckTest.map(
            app.waitFor(obj("id", 2, "cmd", "newtrack", "name", "Keys")).get("result")).get("id");
        assertJson(obj("deleted", false), app.waitFor(obj("id", 3, "cmd", "deletetrack", "trackId", plain)).get("result"));
        // Nor one you renamed after it came in.
        Map<String, Object> made = SelfCheckTest.map(
            app.waitFor(obj("id", 4, "cmd", "newtrack", "name", "Warm Pad", "audition", true)).get("result"));
        int at = ((Number) made.get("index")).intValue();
        bitwig.state.tracks.get(at).name = "My Pad";
        bitwig.refresh();
        assertJson(obj("deleted", false),
                   app.waitFor(obj("id", 6, "cmd", "deletetrack", "trackId", made.get("id"))).get("result"));
        assertEquals(before + 2, bitwig.state.tracks.size());
        assertFalse(bitwig.calls.stream().anyMatch(c -> c.get(0).equals("deleteTrack")), String.valueOf(bitwig.calls));
        assertEquals("Say which track, by its ID.", error(app.waitFor(obj("id", 7, "cmd", "deletetrack"))));
    }

    @Test
    @DisplayName("still takes out an audition track once the app that asked has closed (extension 3.21)")
    void deletesAnAuditionTrackAfterTheAppHasClosed() {
        // The app sends deletetrack as it closes, and may give up waiting while a slow change runs in front of it.
        Model m = LinkTest.song();
        m.newTrackIds = true;
        start(m);
        bitwig.allowChanges(true);
        int before = bitwig.state.tracks.size();
        String id = (String) SelfCheckTest.map(
            app.waitFor(obj("id", 1, "cmd", "newtrack", "name", "Warm Pad", "audition", true)).get("result")).get("id");
        bitwig.model.clipDelay = 1000;
        app.send(obj("id", 2, "cmd", "clip", "trackId", id, "path", "C:\\MIDI\\MO Play 1.mid", "launch", true, "audition", true));
        app.send(obj("id", 3, "cmd", "deletetrack", "trackId", id, "transport", true));
        app.close();
        bitwig.clock.advance(10000);
        assertEquals(before, bitwig.state.tracks.size());
    }
}
