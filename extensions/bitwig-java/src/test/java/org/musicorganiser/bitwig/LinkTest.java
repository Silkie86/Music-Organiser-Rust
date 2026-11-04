package org.musicorganiser.bitwig;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertSame;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.musicorganiser.bitwig.FakeBitwig.NOTE_TRANSPOSE;
import static org.musicorganiser.bitwig.FakeBitwig.SAMPLER;
import static org.musicorganiser.bitwig.FakeBitwig.action;
import static org.musicorganiser.bitwig.FakeBitwig.device;
import static org.musicorganiser.bitwig.FakeBitwig.knob;
import static org.musicorganiser.bitwig.FakeBitwig.track;
import static org.musicorganiser.bitwig.Messages.assertJson;
import static org.musicorganiser.bitwig.Messages.assertMatches;
import static org.musicorganiser.bitwig.Messages.at;
import static org.musicorganiser.bitwig.Messages.error;
import static org.musicorganiser.bitwig.Messages.list;
import static org.musicorganiser.bitwig.Messages.listAt;
import static org.musicorganiser.bitwig.Messages.obj;

import com.bitwig.extension.ExtensionDefinition;
import com.bitwig.extension.api.PlatformType;
import com.bitwig.extension.controller.AutoDetectionMidiPortNamesList;
import com.bitwig.extension.controller.api.ControllerHost;
import java.io.InputStream;
import java.net.URL;
import java.net.URLClassLoader;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.Map;
import java.util.ServiceLoader;
import java.util.UUID;
import java.util.function.Consumer;
import java.util.jar.JarEntry;
import java.util.jar.JarOutputStream;
import java.util.stream.Collectors;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;
import org.musicorganiser.bitwig.FakeBitwig.App;
import org.musicorganiser.bitwig.FakeBitwig.Model;
import org.musicorganiser.bitwig.FakeBitwig.TrackSpec;

/**
 * Runs the extension in a stand-in Bitwig (FakeBitwig) and talks to it the way the app does: every scenario of
 * the JavaScript script's tests (extensions/bitwig/test/link.test.js), under the same names, so the two links
 * are held to the same behaviour.
 */
class LinkTest {
    static final String CHANGES_OFF = "Changes are turned off";
    /** The stand-in's Drum Machine; Bitwig's own ID isn't known. */
    static final String DRUM_MACHINE = "d7d7d7d7-0000-4000-8000-000000000001";

    @TempDir
    Path reports;

    FakeBitwig start(Model model) {
        return FakeBitwig.start(model, reports);
    }

    static Model song() {
        Model m = new Model();
        m.project = "Night Drive";
        m.tempo = 124;
        m.timeSignature = "7/8";
        m.selectedTrack = "Bass";
        m.tracks(
            track("Drums", "Group").group(),
            track("Bass", "Instrument",
                  device("Serum").preset("Reese").plugin().kind("note_to_audio"),
                  device("EQ+").disabled())
                .colour(1, 0.5f, 0).volume(0.6, "-6.0 dB"),
            track("Master", "Master", device("Peak Limiter")));
        m.actions.add(action("Save", "Save", "File"));
        m.actions.add(action("toggle_metronome", "Toggle Metronome", "Transport"));
        return m;
    }

    @Test
    @DisplayName("is the same controller to Bitwig as the script was")
    void isTheSameControllerAsTheScript() {
        // Bitwig finds the extension through META-INF/services, as it will in the .bwextension.
        List<ExtensionDefinition> found = new ArrayList<>();
        ServiceLoader.load(ExtensionDefinition.class).forEach(found::add);
        assertEquals(1, found.size());
        MusicOrganiserExtensionDefinition definition = (MusicOrganiserExtensionDefinition) found.get(0);

        // The script's identity (MusicOrganiser.control.js's defineController), so Bitwig keeps the controller it has.
        assertEquals(List.of("Music Organiser", "DAW link", "Music Organiser", "Music Organiser", MusicOrganiserExtensionDefinition.VERSION),
                     List.of(definition.getHardwareVendor(), definition.getHardwareModel(), definition.getName(), definition.getAuthor(),
                             definition.getVersion()));
        assertEquals(UUID.fromString("ed35e3c7-c544-49ed-936d-c236b0c9b73b"), definition.getId());
        assertEquals(4, definition.getId().version());
        assertEquals(23, definition.getRequiredAPIVersion());
        // Extension 3.7: one MIDI in and one out, for the loopback, which Bitwig offers to set to the loopMIDI port of that name.
        assertEquals(List.of(1, 1), List.of(definition.getNumMidiInPorts(), definition.getNumMidiOutPorts()));
        for (PlatformType platform : PlatformType.values()) {
            AutoDetectionMidiPortNamesList ports = new AutoDetectionMidiPortNamesList();
            definition.listAutoDetectionMidiPortNames(ports, platform);
            assertEquals(1, ports.getCount());
            assertEquals(List.of("Music Organiser Bitwig"), List.of(ports.getPortNamesAt(0).getInputNames()));
            assertEquals(List.of("Music Organiser Bitwig"), List.of(ports.getPortNamesAt(0).getOutputNames()));
        }

        // Making it touches nothing in Bitwig until Bitwig starts it.
        FakeBitwig bitwig = start(song());
        assertTrue(definition.createInstance((ControllerHost) bitwig.host.api) instanceof MusicOrganiserExtension);
    }

    @Test
    @DisplayName("says hello when the app connects")
    void saysHello() {
        FakeBitwig bitwig = start(song());
        App app = bitwig.connect();
        List<Map<String, Object>> fresh = app.take();
        assertEquals(1, fresh.size());
        Map<String, Object> hello = fresh.get(0);
        assertEquals("hello", hello.get("event"));
        assertJson(obj("link", 3, "product", "Bitwig Studio", "version", "6.1.2", "api", 25, "platform", "WINDOWS", "port", 19791,
                       "project", "Night Drive", "modified", false, "engine", true, "changes", false,
                       "onlyIn", true,   // it honours a request's onlyIn
                       // Link version 3: which link this is.
                       "runtime", "java", "extension", MusicOrganiserExtensionDefinition.VERSION,
                       // Extension 3.7: whether the link's own MIDI has come back through the loopback (it isn't set up here).
                       "loopback", false),
                   hello.get("result"));
        assertJson(obj("id", 1, "ok", true, "result", hello.get("result")), app.ask(obj("id", 1, "cmd", "hello")));
        assertEquals(List.of(), bitwig.popups);
        assertEquals(List.of(), bitwig.errors);
    }

    @Test
    @DisplayName("says which file Bitwig loaded it from, as Bitwig may load extensions from another folder than its projects'")
    void saysWhereItWasLoadedFrom() {
        Model model = song();
        model.loadedFrom = "C:\\Users\\me\\Documents\\Bitwig Studio\\Extensions\\MusicOrganiser.bwextension";
        FakeBitwig bitwig = start(model);
        App app = bitwig.connect();
        Map<String, Object> hello = app.take().get(0);
        assertEquals(model.loadedFrom, ((Map<?, ?>) hello.get("result")).get("loadedFrom"));

        // A class from an archive gives its file; the extension's own classes here come from a folder, which isn't one.
        Path junit = Path.of(Link.loadedFrom(Test.class));
        assertTrue(junit.getFileName().toString().endsWith(".jar"), junit.toString());
        assertEquals("", Link.loadedFrom(Link.class));
    }

    /** A class with nothing it needs, for loading from an archive of its own. */
    public static final class Loaded {}

    @Test
    @DisplayName("says where it was loaded from when Bitwig's URL for it isn't escaped (a folder with a space in its name)")
    @SuppressWarnings("deprecation")   // File.toURL, as Bitwig uses it
    void saysWhereItWasLoadedFromAFolderWithASpace(@TempDir Path dir) throws Exception {
        Path extension = Files.createDirectories(dir.resolve("Bitwig Studio").resolve("Extensions")).resolve("MusicOrganiser.bwextension");
        String name = Loaded.class.getName().replace('.', '/') + ".class";
        try (InputStream in = Loaded.class.getClassLoader().getResourceAsStream(name);
             JarOutputStream jar = new JarOutputStream(Files.newOutputStream(extension))) {
            jar.putNextEntry(new JarEntry(name));
            in.transferTo(jar);
        }
        URL url = extension.toFile().toURL();
        assertTrue(url.toString().contains("Bitwig Studio"), url.toString());   // not escaped, so not a URI
        try (URLClassLoader bitwig = new URLClassLoader(new URL[] {url}, ClassLoader.getPlatformClassLoader())) {
            Class<?> loaded = bitwig.loadClass(Loaded.class.getName());
            assertEquals(extension.toString(), Link.loadedFrom(loaded));
        }
    }

    @Test
    @DisplayName("warns in Bitwig when its port is taken")
    void warnsWhenThePortIsTaken() {
        Model m = new Model();
        m.portInUse = 50123;
        FakeBitwig bitwig = start(m);
        assertMatches("port 19791 is taken", bitwig.popups.get(0));
        // Most often by the link itself, loaded twice.
        assertMatches("listed twice in Bitwig's Settings > Controllers, remove one", bitwig.popups.get(0));
        assertEquals(List.of("Music Organiser: waiting for the app on port 50123."), bitwig.printed);
    }

    @Test
    @DisplayName("doesn't say the port is taken while Bitwig hasn't said which port it gave")
    void waitsForBitwigToSayWhichPort() {
        // Bitwig 6.1.3 says -1 straight after making the connection, and the app connects on 19791 all the same.
        Model m = song();
        m.portUnknown = 1000;
        FakeBitwig bitwig = start(m);
        bitwig.clock.advance(2000);
        assertEquals(List.of(), bitwig.popups);
        assertEquals(List.of("Music Organiser: waiting for the app on port 19791."), bitwig.printed);
        App app = bitwig.connect();
        assertEquals(19791, ((Number) at(app.take().get(0), "result", "port")).intValue());
        assertEquals(List.of(), bitwig.popups);
    }

    @Test
    @DisplayName("says the port is taken once Bitwig says it gave another")
    void warnsOnceBitwigSaysWhichPort() {
        Model m = song();
        m.portUnknown = 1;
        m.portInUse = 50123;
        FakeBitwig bitwig = start(m);
        assertEquals(List.of(), bitwig.popups);
        bitwig.clock.advance(1000);
        assertEquals(1, bitwig.popups.size());
        assertMatches("port 19791 is taken", bitwig.popups.get(0));
        bitwig.connect().take();
        assertEquals(1, bitwig.popups.size());
    }

    @Test
    @DisplayName("lists the project's tracks and their devices")
    void listsTracksAndDevices() {
        FakeBitwig bitwig = start(song());
        App app = bitwig.connect();
        app.take();
        Map<String, Object> reply = app.ask(obj("id", "p", "cmd", "project"));
        assertEquals(true, reply.get("ok"));
        Object p = reply.get("result");
        assertJson(list("Night Drive", 124, "7/8", 3), list(at(p, "name"), at(p, "tempo"), at(p, "timeSignature"), at(p, "trackCount")));
        assertEquals(List.of("Drums", "Bass", "Master"), names(listAt(p, "tracks")));
        Object bass = at(p, "tracks", 1);
        assertJson(obj("index", 1, "type", "Instrument", "group", false, "color", "#ff8000", "volume", "-6.0 dB", "volumeValue", 0.6),
                   obj("index", at(bass, "index"), "type", at(bass, "type"), "group", at(bass, "group"), "color", at(bass, "color"),
                       "volume", at(bass, "volume"), "volumeValue", at(bass, "volumeValue")));
        assertJson(list(obj("index", 0, "name", "Serum", "preset", "Reese", "kind", "note_to_audio", "plugin", true, "enabled", true),
                        obj("index", 1, "name", "EQ+", "preset", "", "kind", "audio_to_audio", "plugin", false, "enabled", false)),
                   at(bass, "devices"));
        assertJson(2, at(bass, "deviceCount"));
        assertEquals(true, at(p, "tracks", 0, "group"));
    }

    @Test
    @DisplayName("waits for the selected device's parameter texts")
    void waitsForParameterTexts() {
        FakeBitwig bitwig = start(song());
        bitwig.cursorDevice.show(device("Serum").preset("Reese").plugin(),
                                 knob("CONTENTS/PID3e8", "Cutoff", 0.5, "440 Hz"),
                                 knob("CONTENTS/PID3e9", "Mix", Double.NaN, "50 %"));
        App app = bitwig.connect();
        app.take();

        app.send(obj("id", 7, "cmd", "device"));
        assertEquals(List.of(), app.take(), "answers only once the texts are in");
        assertArrayEquals(new String[] {"CONTENTS/PID3e8", "CONTENTS/PID3e9"}, bitwig.cursorDevice.observed);
        bitwig.clock.advance(100);
        List<Map<String, Object>> replies = app.take();
        assertEquals(1, replies.size());
        Map<String, Object> reply = replies.get(0);
        assertJson(7, reply.get("id"));
        assertEquals("Bass", at(reply, "result", "track"));
        assertEquals("Serum", at(reply, "result", "device", "name"));
        assertJson(list(obj("id", "CONTENTS/PID3e8", "name", "Cutoff", "value", 0.5, "display", "440 Hz"),
                        obj("id", "CONTENTS/PID3e9", "name", "Mix", "value", null, "display", "50 %")),
                   at(reply, "result", "params"));
        assertNull(bitwig.cursorDevice.observed, "stops watching the texts afterwards");
    }

    @Test
    @DisplayName("gives up on parameter texts that never come")
    void givesUpOnParameterTexts() {
        FakeBitwig bitwig = start(song());
        bitwig.cursorDevice.show(device("Odd Plugin").plugin(), knob("a", "Talks", 1, "on"), knob("b", "Silent", 0, null));
        App app = bitwig.connect();
        app.take();
        app.send(obj("id", 1, "cmd", "device"));
        app.send(obj("id", 2, "cmd", "device"));   // asked twice while waiting: both get the answer
        bitwig.clock.advance(1900);
        assertEquals(List.of(), app.take());
        bitwig.clock.advance(200);
        List<Map<String, Object>> replies = app.take();
        assertJson(list(1, 2), replies.stream().map(r -> r.get("id")).collect(Collectors.toList()));
        assertJson(list("on", null), listAt(replies.get(0), "result", "params").stream().map(p -> at(p, "display")).collect(Collectors.toList()));
    }

    @Test
    @DisplayName("says when no device is selected")
    void saysWhenNoDeviceIsSelected() {
        FakeBitwig bitwig = start(song());
        App app = bitwig.connect();
        app.take();
        assertJson(obj("track", "Bass", "device", null, "params", list(), "drumPads", false), app.ask(obj("id", 3, "cmd", "device")).get("result"));
    }

    @Test
    @DisplayName("forgets parameters of the previous device")
    void forgetsThePreviousDevicesParameters() {
        FakeBitwig bitwig = start(song());
        bitwig.cursorDevice.show(device("One"), knob("x", "Old", 0.1, "1"));
        bitwig.cursorDevice.show(device("Two"), knob("y", "New", 0.2, "2"));
        App app = bitwig.connect();
        app.take();
        app.send(obj("id", 1, "cmd", "device"));
        bitwig.clock.advance(100);
        assertJson(list(obj("id", "y", "name", "New", "value", 0.2, "display", "2")), at(app.take().get(0), "result", "params"));
    }

    @Test
    @DisplayName("changes nothing until changes are allowed in Bitwig")
    void changesNothingUntilAllowed() {
        FakeBitwig bitwig = start(song());
        bitwig.cursorDevice.show(device("Serum"), knob("p", "Cutoff", 0, null));
        App app = bitwig.connect();
        app.take();
        for (Map<String, Object> request : List.of(
                obj("id", 1, "cmd", "set", "param", "p", "value", 0.5),
                obj("id", 2, "cmd", "insert", "kind", "vst3", "plugin", "x"),
                obj("id", 3, "cmd", "action", "action", "Save"),
                obj("id", 4, "cmd", "newtrack", "name", "Lead"),
                obj("id", 5, "cmd", "clip", "track", 1, "path", "C:\\Lead.mid"),
                obj("id", 6, "cmd", "pads", "pads", list(obj("note", 36, "path", "C:\\Kick.wav"))),
                obj("id", 7, "cmd", "transpose", "track", 1, "device", 0, "semitones", 2),
                obj("id", 8, "cmd", "recorder", "do", "start"),
                obj("id", 9, "cmd", "arrange", "track", 1, "slot", 0, "at", 16))) {
            assertMatches(CHANGES_OFF, error(app.ask(request)));
        }
        assertEquals(List.of(), bitwig.calls);
        assertEquals(List.of(), bitwig.cursorDevice.setCalls);
    }

    @Test
    @DisplayName("sets the selected device's parameters")
    void setsParameters() {
        FakeBitwig bitwig = start(song());
        bitwig.allowChanges(true);
        bitwig.cursorDevice.show(device("Serum"), knob("CONTENTS/PID3e8", "Cutoff", 0, null));
        App app = bitwig.connect();
        assertEquals(true, at(app.take().get(0), "result", "changes"));

        assertEquals(true, app.ask(obj("id", 1, "cmd", "set", "param", "CONTENTS/PID3e8", "value", 0.25)).get("ok"));
        assertEquals(List.of(Arrays.asList("CONTENTS/PID3e8", 250000L, 1000001L)), bitwig.cursorDevice.setCalls);
        assertMatches("no parameter nope", error(app.ask(obj("id", 2, "cmd", "set", "param", "nope", "value", 0.25))));
        assertMatches("from 0 to 1", error(app.ask(obj("id", 3, "cmd", "set", "param", "CONTENTS/PID3e8", "value", 2))));
        assertMatches("from 0 to 1", error(app.ask(obj("id", 4, "cmd", "set", "param", "CONTENTS/PID3e8", "value", "0.5"))));
    }

    @Test
    @DisplayName("inserts plugins, Bitwig devices, presets and samples")
    void insertsOnTheSelectedTrack() {
        FakeBitwig bitwig = start(song());
        bitwig.allowChanges(true);
        App app = bitwig.connect();
        app.take();
        Consumer<Map<String, Object>> ok = request -> assertEquals(true, app.ask(request).get("ok"), Json.write(request));
        ok.accept(obj("id", 1, "cmd", "insert", "kind", "vst3", "plugin", "805ECA49563E43EA8119369F0ED6096D"));
        ok.accept(obj("id", 2, "cmd", "insert", "kind", "clap", "plugin", "org.musicorganiser.probe.effect"));
        ok.accept(obj("id", 3, "cmd", "insert", "kind", "vst2", "plugin", 1299137074));
        ok.accept(obj("id", 4, "cmd", "insert", "kind", "bitwig", "device", "c9a8b2f5-4f50-4f5c-98f2-a7b3c1e0d1aa"));
        ok.accept(obj("id", 5, "cmd", "insert", "kind", "file", "path", "C:\\Samples\\Café\\Kick ß.wav"));
        assertEquals(List.of(
            Arrays.asList("insertVST3Device", "805ECA49563E43EA8119369F0ED6096D"),
            Arrays.asList("insertCLAPDevice", "org.musicorganiser.probe.effect"),
            Arrays.asList("insertVST2Device", 1299137074),
            Arrays.asList("insertBitwigDevice", UUID.fromString("c9a8b2f5-4f50-4f5c-98f2-a7b3c1e0d1aa")),
            Arrays.asList("insertFile", "C:\\Samples\\Café\\Kick ß.wav")), bitwig.calls);
        assertMatches("is a number", error(app.ask(obj("id", 6, "cmd", "insert", "kind", "vst2", "plugin", "MoB2"))));
        assertMatches("use vst3, clap, vst2, bitwig or file", error(app.ask(obj("id", 7, "cmd", "insert", "kind", "aax", "plugin", "x"))));
        assertMatches("No plugin ID given", error(app.ask(obj("id", 9, "cmd", "insert", "kind", "vst3"))));
        assertMatches("No file", error(app.ask(obj("id", 8, "cmd", "insert", "kind", "file", "path", ""))));
    }

    @Test
    @DisplayName("lists and runs Bitwig's actions")
    void listsAndRunsActions() {
        FakeBitwig bitwig = start(song());
        App app = bitwig.connect();
        app.take();
        assertJson(list(obj("id", "Save", "name", "Save", "category", "File", "menu", "Save"),
                        obj("id", "toggle_metronome", "name", "Toggle Metronome", "category", "Transport", "menu", "Toggle Metronome")),
                   app.ask(obj("id", 1, "cmd", "actions")).get("result"));
        bitwig.allowChanges(true);
        // The answer names the project Bitwig had in front as the action ran, and the route it ran by: without one asked for,
        // from Bitwig's flush (extension 3.8), which Save worked by in Bitwig 6.1.3 where straight away it didn't.
        assertJson(obj("id", 2, "ok", true, "result", obj("project", "Night Drive", "via", "flush", "ran", true)),
                   app.waitFor(obj("id", 2, "cmd", "action", "action", "Save")));
        assertEquals(List.of(List.of("invoke", "Save")), bitwig.calls);
        assertMatches("no action Format C:", error(app.ask(obj("id", 3, "cmd", "action", "action", "Format C:"))));
        bitwig.projectName.v = "My Song";
        assertJson(obj("project", "My Song", "via", "direct", "ran", true),
                   app.ask(obj("id", 4, "cmd", "action", "action", "toggle_metronome", "via", "direct")).get("result"));
    }

    @Test
    @DisplayName("still runs an action when it can't read the project's name, and leaves the name out")
    void runsAnActionWithoutTheProjectsName() {
        FakeBitwig bitwig = start(song());
        bitwig.allowChanges(true);
        App app = bitwig.connect();
        app.take();
        // As if the application part of Bitwig's API had only half set up: its project name can't be read.
        bitwig.projectName.interested = false;
        assertJson(obj("id", 1, "ok", true, "result", obj("via", "direct", "ran", true)),
                   app.ask(obj("id", 1, "cmd", "action", "action", "Save", "via", "direct")));
        assertEquals(List.of(List.of("invoke", "Save")), bitwig.calls);
    }

    @Test
    @DisplayName("selects a track and one of its devices")
    void selectsATrackAndDevice() {
        FakeBitwig bitwig = start(song());
        App app = bitwig.connect();
        app.take();
        assertEquals(true, app.ask(obj("id", 1, "cmd", "select", "track", 1, "device", 1)).get("ok"));
        assertEquals("selectChannel", bitwig.calls.get(0).get(0));
        assertSame(bitwig.track(1), bitwig.calls.get(0).get(1));
        assertEquals(List.of(bitwig.track(1).devices.items.get(1)), bitwig.cursorDevice.selected);
        assertMatches("no track 3", error(app.ask(obj("id", 2, "cmd", "select", "track", 3))));
        assertMatches("no track 0.5", error(app.ask(obj("id", 3, "cmd", "select", "track", 0.5))));
        assertMatches("has no device 2", error(app.ask(obj("id", 4, "cmd", "select", "track", 1, "device", 2))));
    }

    @Test
    @DisplayName("answers requests it can't follow with an error")
    void answersWhatItCantFollowWithAnError() {
        FakeBitwig bitwig = start(song());
        App app = bitwig.connect();
        app.take();
        assertJson(obj("id", null, "ok", false, "error", "The request isn't JSON: {not json"), app.ask("{not json"));
        assertJson(obj("id", 2, "ok", false, "error", "Unknown command: explode"), app.ask(obj("id", 2, "cmd", "explode")));
        assertEquals("Unknown command: toString", error(app.ask(obj("id", 3, "cmd", "toString"))));
        assertEquals("Unknown command: undefined", error(app.ask(obj("id", 4))));
        assertEquals("Unknown command: null", error(app.ask("null")));
    }

    @Test
    @DisplayName("keeps text outside ASCII intact")
    void keepsTextOutsideAscii() {
        String name = "Café ☕ 𝄞 Ünïcödé";
        Model m = song();
        m.tracks.clear();
        m.tracks(track(name, null));
        FakeBitwig bitwig = start(m);
        App app = bitwig.connect();
        app.take();
        assertEquals(name, at(app.ask(obj("id", 1, "cmd", "project")), "result", "tracks", 0, "name"));
    }

    @Test
    @DisplayName("stops sending to an app that disconnected")
    void stopsSendingAfterADisconnect() {
        FakeBitwig bitwig = start(song());
        bitwig.cursorDevice.show(device("Serum"), knob("p", "Cutoff", 0, "0"));
        App app = bitwig.connect();
        app.take();
        app.send(obj("id", 1, "cmd", "device"));
        app.close();
        bitwig.clock.advance(100);
        assertEquals(List.of(), app.take());
        assertEquals(List.of(), bitwig.errors);
    }

    @Test
    @DisplayName("stops sending to an app whose connection broke, and says so in Bitwig")
    void stopsSendingOnABrokenConnection() {
        FakeBitwig bitwig = start(song());
        App app = bitwig.connect();
        app.take();
        App other = bitwig.connect();
        other.take();
        app.breakConnection();
        app.send(obj("id", 1, "cmd", "hello"));
        app.send(obj("id", 2, "cmd", "hello"));
        assertEquals(List.of(), app.take());
        assertEquals(1, bitwig.errors.size(), "says so once");
        assertMatches("^Music Organiser: lost the connection to the app \\(", bitwig.errors.get(0));
        assertEquals(true, other.ask(obj("id", 3, "cmd", "hello")).get("ok"), "the other connection carries on");
        assertJson(1, at(other.ask(obj("id", 4, "cmd", "selftest")), "result", "connections"));
    }

    // ---- Sending things into Bitwig ----

    FakeBitwig bitwig;
    App app;

    /** A project with effect tracks and changes allowed, ready for the app to send things into. */
    void studio(Consumer<Model> extra) {
        Model m = new Model();
        m.project = "Night Drive";
        m.drumMachine = DRUM_MACHINE;
        m.selectedTrack = "Bass";
        m.tracks(
            track("Bass", "Instrument", device("Serum").plugin().kind("note_to_audio")),
            track("Reverb", "Effect", device("Reverb")),
            track("Master", "Master"));
        m.files.put("C:\\Presets\\Lead.bwpreset", device("Serum").preset("Lead").plugin().kind("note_to_audio"));
        m.plugins.put("VST3:805ECA49563E43EA8119369F0ED6096D", device("Serum").plugin().kind("note_to_audio"));
        extra.accept(m);
        bitwig = start(m);
        bitwig.allowChanges(true);
        app = bitwig.connect();
        app.take();
    }

    void studio() {
        studio(m -> {});
    }

    @Test
    @DisplayName("adds a named, coloured instrument track after the main tracks and selects it")
    void addsATrack() {
        studio();
        Map<String, Object> reply = app.waitFor(obj("id", 1, "cmd", "newtrack", "name", "Lead", "color", "#ff8000"));
        assertJson(obj("id", 1, "ok", true, "result", obj("index", 1, "name", "Lead")), reply);
        List<Object> tracks = listAt(app.ask(obj("id", 2, "cmd", "project")), "result", "tracks");
        assertEquals(List.of(List.of("Bass", "Instrument"), List.of("Lead", "Instrument"), List.of("Reverb", "Effect"), List.of("Master", "Master")),
                     tracks.stream().map(t -> List.of(at(t, "name"), at(t, "type"))).collect(Collectors.toList()));
        assertEquals("#ff8000", at(tracks, 1, "color"));
        assertEquals(List.of(bitwig.track(1)), calls("selectChannel").stream().map(c -> c.get(1)).collect(Collectors.toList()));
        assertEquals("Lead", at(app.ask(obj("id", 3, "cmd", "device")), "result", "track"));

        // Without a name, the track keeps Bitwig's.
        assertJson(obj("index", 2, "name", "Inst 3"), app.waitFor(obj("id", 4, "cmd", "newtrack")).get("result"));
        assertMatches("#rrggbb", error(app.ask(obj("id", 5, "cmd", "newtrack", "color", "orange"))));
        assertMatches("name is text", error(app.ask(obj("id", 6, "cmd", "newtrack", "name", 7))));
    }

    @Test
    @DisplayName("closes Bitwig's browser if a new track opens it")
    void closesTheBrowser() {
        studio(m -> m.browserOnNewTrack = true);
        assertEquals(true, app.waitFor(obj("id", 1, "cmd", "newtrack", "name", "Lead")).get("ok"));
        assertEquals(List.of(List.of("cancelBrowser")), calls("cancelBrowser"));
    }

    @Test
    @DisplayName("says when Bitwig adds a track where the app can't see it")
    void saysWhenANewTrackIsOutOfReach() {
        studio(m -> {
            m.tracks.clear();
            for (int i = 0; i < 128; i++) m.tracks(track("Track " + (i + 1), null));
        });
        assertMatches("past the first 128 tracks", error(app.waitFor(obj("id", 1, "cmd", "newtrack", "name", "Lead"))));
    }

    @Test
    @DisplayName("inserts onto a given track and says whether Bitwig added a device")
    void insertsOnAGivenTrack() {
        studio();
        Map<String, Object> sample = app.waitFor(obj("id", 1, "cmd", "insert", "track", 0, "kind", "file", "path", "C:\\Samples\\Pluck.wav"));
        assertJson(obj("added", true, "index", 1,
                       "device", obj("index", 1, "name", "Sampler", "preset", "Pluck", "kind", "note_to_audio", "plugin", false, "enabled", true)),
                   sample.get("result"));
        Map<String, Object> first = app.waitFor(obj("id", 2, "cmd", "insert", "track", 0, "at", "start", "kind", "bitwig", "device", NOTE_TRANSPOSE));
        assertJson(list(true, 0, "Note Transpose"), list(at(first, "result", "added"), at(first, "result", "index"), at(first, "result", "device", "name")));
        assertEquals(List.of("Note Transpose", "Serum", "Sampler"), deviceNames(bitwig.state.tracks.get(0)));

        Map<String, Object> preset = app.waitFor(obj("id", 3, "cmd", "insert", "track", 2, "kind", "file", "path", "C:\\Presets\\Lead.bwpreset"));
        assertEquals(List.of("Serum", "Lead"), List.of(at(preset, "result", "device", "name"), at(preset, "result", "device", "preset")));

        // Bitwig ignores what it can't use, so after waiting the answer says nothing was added.
        Map<String, Object> unknown = app.waitFor(obj("id", 4, "cmd", "insert", "track", 0, "kind", "vst3", "plugin", "00000000000000000000000000000000"));
        assertJson(obj("added", false, "index", null, "device", null), unknown.get("result"));
        assertMatches("no track 9", error(app.ask(obj("id", 5, "cmd", "insert", "track", 9, "kind", "file", "path", "x.wav"))));
        assertMatches("\"start\" or the \"end\"", error(app.ask(obj("id", 6, "cmd", "insert", "track", 0, "at", "middle", "kind", "file", "path", "x.wav"))));
        assertMatches("Say which track", error(app.ask(obj("id", 7, "cmd", "insert", "at", "start", "kind", "file", "path", "x.wav"))));
    }

    @Test
    @DisplayName("sets the pitch of a Sampler or a Note Transpose at the start of a track")
    void setsThePitch() {
        studio();
        app.waitFor(obj("id", 1, "cmd", "newtrack", "name", "Pluck"));
        app.waitFor(obj("id", 2, "cmd", "insert", "track", 1, "kind", "file", "path", "C:\\Samples\\Pluck.wav"));
        assertJson(obj("device", "Sampler", "value", -3.5, "display", "-3.50 st"),
                   app.waitFor(obj("id", 3, "cmd", "transpose", "track", 1, "device", 0, "semitones", -3.5)).get("result"));
        assertEquals(-3.5, bitwig.state.tracks.get(1).devices.get(0).params.get("TRANSPOSE"));

        app.waitFor(obj("id", 4, "cmd", "insert", "track", 0, "at", "start", "kind", "bitwig", "device", NOTE_TRANSPOSE));
        assertJson(obj("device", "Note Transpose", "value", 7, "display", "+7 st"),
                   app.waitFor(obj("id", 5, "cmd", "transpose", "track", 0, "device", 0, "semitones", 7)).get("result"));
        // What Bitwig ends up with comes back, whatever was asked for.
        assertJson(48, at(app.waitFor(obj("id", 6, "cmd", "transpose", "track", 0, "device", 0, "semitones", 60)), "result", "value"));

        assertMatches("neither a Sampler nor a Note Transpose", error(app.waitFor(obj("id", 7, "cmd", "transpose", "track", 0, "device", 1, "semitones", 2))));
        assertMatches("first 2 devices", error(app.ask(obj("id", 8, "cmd", "transpose", "track", 0, "device", 2, "semitones", 2))));
        assertMatches("how many semitones", error(app.ask(obj("id", 9, "cmd", "transpose", "track", 1, "device", 0, "semitones", "up"))));
    }

    @Test
    @DisplayName("puts a MIDI file in the track's first empty clip slot")
    void putsAMidiFileInAClipSlot() {
        studio(m -> m.scenes = 2);
        bitwig.state.tracks.get(0).slots("old.mid");
        bitwig.refresh();
        assertJson(obj("slot", 1), app.waitFor(obj("id", 1, "cmd", "clip", "track", 0, "path", "C:\\MIDI\\Bass.mid")).get("result"));
        // With the project's scenes full, the next clip starts a new scene.
        assertJson(obj("slot", 2), app.waitFor(obj("id", 2, "cmd", "clip", "track", 0, "path", "C:\\MIDI\\Bass 2.mid")).get("result"));
        assertEquals(List.of("old.mid", "C:\\MIDI\\Bass.mid", "C:\\MIDI\\Bass 2.mid"), bitwig.state.tracks.get(0).slots);
        assertEquals(List.of("track 0 slot 1", "track 0 slot 2"),
                     bitwig.calls.stream().filter(c -> c.size() > 2 && String.valueOf(c.get(2)).contains("slot")).map(c -> c.get(2))
                                 .collect(Collectors.toList()));

        assertMatches("already has a clip", error(app.ask(obj("id", 3, "cmd", "clip", "track", 0, "slot", 0, "path", "C:\\MIDI\\x.mid"))));
        assertMatches("first 8 clip slots", error(app.ask(obj("id", 4, "cmd", "clip", "track", 0, "slot", 8, "path", "C:\\MIDI\\x.mid"))));
        assertMatches("didn't make a clip", error(app.waitFor(obj("id", 5, "cmd", "clip", "track", 0, "path", "C:\\notes.txt"))));
        bitwig.state.scenes = 8;
        bitwig.state.tracks.get(0).slots.clear();
        for (int s = 0; s < 8; s++) bitwig.state.tracks.get(0).slots("x.mid");
        bitwig.refresh();
        assertMatches("clip slots are full", error(app.ask(obj("id", 6, "cmd", "clip", "track", 0, "path", "C:\\MIDI\\y.mid"))));
    }

    @Test
    @DisplayName("fills a new Drum Machine's empty pads, from the lowest note")
    void fillsANewDrumMachine() {
        studio(m -> m.padScroll = 36);
        app.waitFor(obj("id", 1, "cmd", "newtrack", "name", "Kit #7"));
        Map<String, Object> machine = app.waitFor(obj("id", 2, "cmd", "insert", "track", 1, "kind", "bitwig", "device", DRUM_MACHINE));
        assertEquals("Drum Machine", at(machine, "result", "device", "name"));

        List<Object> pads = list(
            obj("note", 36, "path", "C:\\Kit\\Kick.wav", "name", "Kick"),
            obj("note", 38, "path", "C:\\Kit\\Snare.wav", "name", "Snare"),
            obj("note", 42, "path", "C:\\Kit\\notes.txt", "name", "Closed hat"));
        Map<String, Object> reply = app.waitFor(obj("id", 3, "cmd", "pads", "track", 1, "device", 0, "pads", pads));
        assertJson(obj("filled", list(36, 38), "replaced", list(), "skipped", list(), "failed", list(42), "taken", list()), reply.get("result"));
        assertFalse(bitwig.pads.indication, "doesn't light up the pads in Bitwig");
        Map<Integer, FakeBitwig.PadSpec> filled = bitwig.state.tracks.get(1).devices.get(0).pads;
        assertEquals(List.of(36, 38), new ArrayList<>(filled.keySet()));
        assertEquals(List.of("Kick", "C:\\Kit\\Kick.wav"), List.of(filled.get(36).name, filled.get(36).devices.get(0).sample));

        // A pad with something on it is left alone.
        Map<String, Object> again = app.waitFor(obj("id", 4, "cmd", "pads", "track", 1, "device", 0,
                                                    "pads", list(obj("note", 36, "path", "C:\\Kit\\Kick 2.wav"), obj("note", 37, "path", "C:\\Kit\\Rim.wav"))));
        assertJson(obj("filled", list(37), "replaced", list(), "skipped", list(36), "failed", list(),
                       "taken", list(obj("note", 36, "name", "Kick", "devices", 1, "first", "Sampler", "sample", "Kick.wav"))),
                   again.get("result"));
        assertEquals(1, filled.get(36).devices.size());
    }

    @Test
    @DisplayName("puts samples in place of the empty Samplers a default Drum Machine has on its pads, keeping what's after them")
    void fillsADefaultDrumMachinesEmptySamplers() {
        // Solomon's default Drum Machine has an empty Sampler on every pad.
        studio(m -> m.drumPads = List.of(device("Sampler").id(SAMPLER).kind("note_to_audio"), device("Compressor")));
        app.waitFor(obj("id", 1, "cmd", "newtrack", "name", "Kit #8"));
        app.waitFor(obj("id", 2, "cmd", "insert", "track", 1, "kind", "bitwig", "device", DRUM_MACHINE));
        List<Object> kit = list(obj("note", 36, "path", "C:\\Kit\\Kick.wav", "name", "Kick"), obj("note", 38, "path", "C:\\Kit\\Snare.wav"));
        assertJson(obj("filled", list(36, 38), "replaced", list(36, 38), "skipped", list(), "failed", list(), "taken", list()),
                   app.waitFor(obj("id", 3, "cmd", "pads", "track", 1, "device", 0, "pads", kit)).get("result"));
        Map<Integer, FakeBitwig.PadSpec> pads = bitwig.state.tracks.get(1).devices.get(0).pads;
        assertEquals(List.of("Sampler", "Compressor"), pads.get(36).devices.stream().map(d -> d.name).collect(Collectors.toList()));
        assertEquals(List.of("Kick", "C:\\Kit\\Kick.wav"), List.of(pads.get(36).name, pads.get(36).devices.get(0).sample));
        assertEquals("C:\\Kit\\Snare.wav", pads.get(38).devices.get(0).sample);
        assertNull(pads.get(40).devices.get(0).sample);

        // A pad whose Sampler has a sample now is left alone.
        assertJson(obj("filled", list(40), "replaced", list(40), "skipped", list(36), "failed", list(),
                       "taken", list(obj("note", 36, "name", "Kick", "devices", 2, "first", "Sampler", "sample", "Kick.wav"))),
                   app.waitFor(obj("id", 4, "cmd", "pads", "track", 1, "device", 0,
                                   "pads", list(obj("note", 36, "path", "C:\\Kit\\Kick 2.wav"), obj("note", 40, "path", "C:\\Kit\\Rim.wav")))).get("result"));
        assertEquals("C:\\Kit\\Kick.wav", pads.get(36).devices.get(0).sample);

        // Nor is one whose empty Sampler has an instrument after it, which plays; effects after it don't. Bitwig 6.1.5
        // calls an instrument "instrument", and (most likely) an audio effect "audio-effect".
        pads.get(42).devices.add(device("Polymer").kind("note_to_audio"));
        pads.get(44).devices.add(device("Delay+"));
        pads.get(46).devices.add(device("EQ+").kind("audio-effect"));
        pads.get(48).devices.add(device("Polymer").kind("instrument"));
        bitwig.clock.advance(100);
        assertJson(obj("filled", list(44, 46), "replaced", list(44, 46), "skipped", list(42, 48), "failed", list(),
                       "taken", list(obj("note", 42, "name", "Sampler", "devices", 3, "first", "Sampler", "sample", ""),
                                     obj("note", 48, "name", "Sampler", "devices", 3, "first", "Sampler", "sample", ""))),
                   app.waitFor(obj("id", 5, "cmd", "pads", "track", 1, "device", 0,
                                   "pads", list(obj("note", 42, "path", "C:\\Kit\\Hat.wav"), obj("note", 44, "path", "C:\\Kit\\Clap.wav"),
                                                obj("note", 46, "path", "C:\\Kit\\Tom.wav"), obj("note", 48, "path", "C:\\Kit\\Ride.wav")))).get("result"));
        assertEquals(List.of("Sampler", "Compressor", "Polymer"), pads.get(42).devices.stream().map(d -> d.name).collect(Collectors.toList()));
        assertNull(pads.get(42).devices.get(0).sample);
    }

    @Test
    @DisplayName("reads and sets the knobs of a plugin it put on a track, once that's selected")
    void readsAndSetsTheKnobsOfAnInsertedPlugin() {
        // How the app copies FL Studio's knobs across and checks they took.
        studio(m -> m.plugins.put("VST3:805ECA49563E43EA8119369F0ED6096D", device("Serum").plugin().kind("note_to_audio").knobs(
            knob("CONTENTS/PID0", "Master Volume", 0.5, "50%"),
            knob("CONTENTS/PID3e8", "Filter Cutoff", 0.5, "50%"))));
        Object track = at(app.waitFor(obj("id", 1, "cmd", "newtrack", "name", "Lead")), "result", "index");
        Object added = at(app.waitFor(obj("id", 2, "cmd", "insert", "track", track, "kind", "vst3", "plugin", "805ECA49563E43EA8119369F0ED6096D")),
                          "result", "index");
        app.waitFor(obj("id", 3, "cmd", "select", "track", track, "device", added));
        Object before = app.waitFor(obj("id", 4, "cmd", "device")).get("result");
        assertEquals(List.of("Lead", "Serum"), List.of(at(before, "track"), at(before, "device", "name")));
        assertJson(list(list("Master Volume", 0.5), list("Filter Cutoff", 0.5)),
                   listAt(before, "params").stream().map(p -> list(at(p, "name"), at(p, "value"))).collect(Collectors.toList()));

        assertEquals(true, app.ask(obj("id", 5, "cmd", "set", "param", "CONTENTS/PID3e8", "value", 0.25)).get("ok"));
        bitwig.clock.advance(100);
        List<Object> after = listAt(app.waitFor(obj("id", 6, "cmd", "device")), "result", "params");
        assertJson(list(list("Master Volume", 0.5, "50%"), list("Filter Cutoff", 0.25, "25%")),
                   after.stream().map(p -> list(at(p, "name"), at(p, "value"), at(p, "display"))).collect(Collectors.toList()));
    }

    @Test
    @DisplayName("fills the selected Drum Machine, and says when the selected device isn't one")
    void fillsTheSelectedDrumMachine() {
        studio();
        app.waitFor(obj("id", 1, "cmd", "insert", "track", 0, "at", "start", "kind", "bitwig", "device", DRUM_MACHINE));
        app.waitFor(obj("id", 2, "cmd", "select", "track", 0, "device", 0));
        assertEquals(true, at(app.waitFor(obj("id", 3, "cmd", "device")), "result", "drumPads"));
        assertJson(obj("filled", list(40), "replaced", list(), "skipped", list(), "failed", list(), "taken", list()),
                   app.waitFor(obj("id", 4, "cmd", "pads", "pads", list(obj("note", 40, "path", "C:\\Kit\\Snare 2.wav")))).get("result"));
        assertEquals(List.of(40), new ArrayList<>(bitwig.state.tracks.get(0).devices.get(0).pads.keySet()));

        app.waitFor(obj("id", 5, "cmd", "select", "track", 0, "device", 1));
        assertMatches("selected device isn't a Drum Machine",
                      error(app.waitFor(obj("id", 6, "cmd", "pads", "pads", list(obj("note", 41, "path", "C:\\Kit\\Tom.wav"))))));
        assertMatches("Device 1 on track 0 isn't a Drum Machine",
                      error(app.waitFor(obj("id", 7, "cmd", "pads", "track", 0, "device", 1, "pads", list(obj("note", 41, "path", "C:\\Kit\\Tom.wav"))))));
        for (Object[] wrong : new Object[][] {
            {list(), "No pads given"},
            {list(obj("note", 128, "path", "a.wav")), "from 0 to 127"},
            {list(obj("note", 36, "path", "a.wav"), obj("note", 36, "path", "b.wav")), "Two samples for note 36"},
            {list(obj("note", 36)), "No sample given"},
        }) {
            assertMatches((String) wrong[1], error(app.ask(obj("id", 8, "cmd", "pads", "pads", wrong[0]))));
        }
    }

    @Test
    @DisplayName("makes changes one at a time, in the order asked")
    void makesChangesOneAtATime() {
        studio();
        app.send(obj("id", 1, "cmd", "newtrack", "name", "One"));
        app.send(obj("id", 2, "cmd", "newtrack", "name", "Two"));
        app.send(obj("id", 3, "cmd", "project"));   // reading doesn't wait for changes
        List<Map<String, Object>> fresh = app.take();
        assertEquals(1, fresh.size());
        assertJson(3, fresh.get(0).get("id"));
        bitwig.clock.advance(20000);
        assertJson(list(list(1, 1, "One"), list(2, 2, "Two")),
                   app.take().stream().map(r -> list(r.get("id"), at(r, "result", "index"), at(r, "result", "name"))).collect(Collectors.toList()));
    }

    @Test
    @DisplayName("answers two apps side by side, each with its own answers, and makes their changes in turn")
    void answersTwoAppsSideBySide() {
        // The app looks every 2 seconds on one connection while a send or a check uses another.
        studio();
        App second = bitwig.connect();
        assertEquals("hello", second.take().get(0).get("event"));
        app.send(obj("id", 1, "cmd", "newtrack", "name", "One"));
        second.send(obj("id", 1, "cmd", "newtrack", "name", "Two"));
        assertEquals(true, second.ask(obj("id", 2, "cmd", "project")).get("ok"));
        assertEquals(List.of(), app.take());
        bitwig.clock.advance(20000);
        assertJson(list(obj("id", 1, "ok", true, "result", obj("index", 1, "name", "One"))), app.take());
        assertJson(list(obj("id", 1, "ok", true, "result", obj("index", 2, "name", "Two"))), second.take());
    }

    @Test
    @DisplayName("leaves out a waiting change once its app has closed the connection, so asking again doesn't make it twice")
    void leavesOutAChangeOnceItsAppHasGone() {
        // The app gave up on "Two" (it took too long) and closed its connection, so it will ask for it again (extension 3.18).
        studio();
        App second = bitwig.connect();
        second.take();
        app.send(obj("id", 1, "cmd", "newtrack", "name", "One"));
        app.send(obj("id", 2, "cmd", "newtrack", "name", "Two"));
        second.send(obj("id", 1, "cmd", "newtrack", "name", "Three"));
        app.close();
        bitwig.clock.advance(20000);
        assertJson(list(obj("id", 1, "ok", true, "result", obj("index", 2, "name", "Three"))), second.take());
        assertEquals(List.of("Bass", "One", "Three", "Reverb", "Master"),
                     bitwig.state.tracks.stream().map(t -> t.name).collect(Collectors.toList()));
    }

    // ---- Only in the project asked for ----

    /** The answer to a request that names one project (onlyIn) while Bitwig has another open. */
    static String elsewhere(String open, String asked) {
        return "Bitwig has \"" + open + "\" open, not \"" + asked + "\", so the link stopped before changing anything there.";
    }

    /** Each track as [name, colour], to see nothing was renamed or coloured. */
    private List<List<Object>> looks() {
        return bitwig.state.tracks.stream().map(t -> Arrays.<Object>asList(t.name, t.colour == null ? null : Arrays.toString(t.colour)))
                                  .collect(Collectors.toList());
    }

    /** The tracks as they look after a new track was stopped part way: Bitwig's own new track, nothing renamed or coloured. */
    private static List<List<Object>> untouched() {
        return List.of(Arrays.asList("Bass", null), Arrays.asList("Inst 2", null), Arrays.asList("Reverb", null), Arrays.asList("Master", null));
    }

    /** A request with an id and an onlyIn added. */
    private static Map<String, Object> with(Map<String, Object> request, Object id, Object onlyIn) {
        Map<String, Object> m = obj("id", id);
        m.putAll(request);
        m.put("onlyIn", onlyIn);
        return m;
    }

    @Test
    @DisplayName("makes a change only in the project the request names")
    void makesAChangeOnlyInTheProjectNamed() {
        studio();
        assertJson(obj("id", 1, "ok", true, "result", obj("index", 1, "name", "Lead")),
                   app.waitFor(obj("id", 1, "cmd", "newtrack", "name", "Lead", "color", "#ff8000", "onlyIn", "Night Drive")));
        assertEquals(true, app.ask(obj("id", 2, "cmd", "select", "track", 0, "onlyIn", "Night Drive")).get("ok"));
        assertEquals("#ff8000", at(app.ask(obj("id", 3, "cmd", "project", "onlyIn", "Night Drive")), "result", "tracks", 1, "color"));
        // null is the same as no onlyIn: no check.
        assertJson(obj("slot", 0), app.waitFor(obj("id", 4, "cmd", "clip", "track", 0, "path", "C:\\MIDI\\Bass.mid", "onlyIn", null)).get("result"));
        assertEquals("C:\\MIDI\\Bass.mid", bitwig.state.tracks.get(0).slots.get(0));
    }

    @Test
    @DisplayName("refuses a request meant for another project, and changes nothing")
    void refusesARequestForAnotherProject() {
        studio();
        for (Map<String, Object> request : List.of(
                obj("cmd", "newtrack", "name", "Lead"),
                obj("cmd", "insert", "track", 0, "kind", "file", "path", "C:\\Samples\\Pluck.wav"),
                obj("cmd", "insert", "kind", "vst3", "plugin", "805ECA49563E43EA8119369F0ED6096D"),
                obj("cmd", "clip", "track", 0, "path", "C:\\MIDI\\Bass.mid"),
                obj("cmd", "pads", "track", 0, "device", 0, "pads", list(obj("note", 36, "path", "C:\\Kit\\Kick.wav"))),
                obj("cmd", "transpose", "track", 0, "device", 0, "semitones", 2),
                obj("cmd", "set", "param", "p", "value", 0.5),
                obj("cmd", "action", "action", "Save"),
                obj("cmd", "recorder", "do", "start"),
                obj("cmd", "arrange", "track", 0, "slot", 0, "at", 16),
                obj("cmd", "select", "track", 0, "device", 0),
                obj("cmd", "project"))) {
            assertJson(obj("id", 1, "ok", false, "error", elsewhere("Night Drive", "MO Live Test")), app.waitFor(with(request, 1, "MO Live Test")));
        }
        // The name has to be the same exactly.
        assertEquals(elsewhere("Night Drive", "night drive"), error(app.ask(obj("id", 2, "cmd", "select", "track", 0, "onlyIn", "night drive"))));
        // An onlyIn that isn't a project's name is refused too.
        for (Object onlyIn : list(5, true, obj(), list("Night Drive")))
            assertJson(obj("id", 3, "ok", false, "error", "onlyIn has to be a project's name."), app.ask(obj("id", 3, "cmd", "newtrack", "onlyIn", onlyIn)));
        bitwig.clock.advance(10000);
        assertEquals(List.of(), bitwig.calls);
        assertEquals(List.of(), bitwig.cursorDevice.setCalls);
        assertEquals(List.of(Arrays.asList("Bass", null), Arrays.asList("Reverb", null), Arrays.asList("Master", null)), looks());
    }

    @Test
    @DisplayName("stops a new track part way when another project comes to the front, and the next change still runs")
    void stopsANewTrackWhenAnotherProjectComesToTheFront() {
        studio(m -> m.browserOnNewTrack = true);
        // Just after Bitwig is asked for the track, someone brings their own project to the front.
        bitwig.switchDuring("newtrack", "My Song");
        assertJson(obj("id", 1, "ok", false, "error", elsewhere("My Song", "Night Drive")),
                   app.waitFor(obj("id", 1, "cmd", "newtrack", "name", "Lead", "color", "#ff8000", "onlyIn", "Night Drive")));
        // The tracks Bitwig shows now are My Song's (the stand-in keeps one project, so the track Bitwig had just added
        // shows there too): none is renamed or coloured, the selection stays put, and Bitwig's browser is left alone.
        assertEquals(untouched(), looks());
        assertEquals(List.of(), bitwig.calls.stream().filter(c -> !c.get(0).equals("createInstrumentTrack")).collect(Collectors.toList()));
        assertEquals("Bass", at(app.ask(obj("id", 2, "cmd", "device")), "result", "track"));

        // Back in Night Drive, the next change goes ahead.
        bitwig.projectName.v = "Night Drive";
        assertJson(obj("index", 2, "name", "Pad"), app.waitFor(obj("id", 3, "cmd", "newtrack", "name", "Pad", "onlyIn", "Night Drive")).get("result"));
    }

    @Test
    @DisplayName("stops a new track before naming it when another project comes to the front while the track listing settles")
    void stopsANewTrackWhileTheListingSettles() {
        studio(m -> m.browserOnNewTrack = true);
        // Bitwig shows the new track, and while the extension waits for the listing to stop changing, just before it
        // names, colours and selects the track and closes Bitwig's browser, someone brings their own project to the front.
        bitwig.switchDuring("newtrack", () -> bitwig.state.tracks.stream().anyMatch(t -> t.name.equals("Inst 2")), "My Song");
        assertJson(obj("id", 1, "ok", false, "error", elsewhere("My Song", "Night Drive")),
                   app.waitFor(obj("id", 1, "cmd", "newtrack", "name", "Lead", "color", "#ff8000", "onlyIn", "Night Drive")));
        assertEquals(untouched(), looks());
        assertEquals(List.of("createInstrumentTrack"), bitwig.calls.stream().map(c -> c.get(0)).collect(Collectors.toList()),
                     "nothing selected and the browser not closed");
        assertEquals(true, bitwig.browserOpen.v);
        assertEquals("Bass", at(app.ask(obj("id", 2, "cmd", "device")), "result", "track"));

        bitwig.projectName.v = "Night Drive";
        assertJson(obj("index", 2, "name", "Pad"), app.waitFor(obj("id", 3, "cmd", "newtrack", "name", "Pad", "onlyIn", "Night Drive")).get("result"));
    }

    @Test
    @DisplayName("stops filling pads when another project comes to the front while the samples load")
    void stopsFillingPadsWhenAnotherProjectComesToTheFront() {
        studio();
        app.waitFor(obj("id", 1, "cmd", "insert", "track", 0, "at", "start", "kind", "bitwig", "device", DRUM_MACHINE));
        bitwig.switchDuring("pads", () -> bitwig.calls.stream().anyMatch(c -> c.get(0).equals("insertFile") && c.size() > 2
                                                                              && String.valueOf(c.get(2)).startsWith("pad ")), "My Song");
        List<Object> kit = list(obj("note", 36, "path", "C:\\Kit\\Kick.wav", "name", "Big Kick"), obj("note", 38, "path", "C:\\Kit\\Snare.wav", "name", "Big Snare"));
        assertJson(obj("id", 2, "ok", false, "error", elsewhere("My Song", "Night Drive")),
                   app.waitFor(obj("id", 2, "cmd", "pads", "track", 0, "device", 0, "pads", kit, "onlyIn", "Night Drive")));
        // The samples were on their way when the project changed; the pads keep the names Bitwig gave them.
        Map<Integer, FakeBitwig.PadSpec> pads = bitwig.state.tracks.get(0).devices.get(0).pads;
        assertEquals(List.of(List.of(36, "Kick"), List.of(38, "Snare")),
                     pads.entrySet().stream().map(e -> List.<Object>of(e.getKey(), e.getValue().name)).collect(Collectors.toList()));

        bitwig.projectName.v = "Night Drive";
        assertJson(obj("filled", list(40), "replaced", list(), "skipped", list(), "failed", list(), "taken", list()),
                   app.waitFor(obj("id", 3, "cmd", "pads", "track", 0, "device", 0, "pads", list(obj("note", 40, "path", "C:\\Kit\\Tom.wav", "name", "Low Tom")),
                                   "onlyIn", "Night Drive")).get("result"));
        assertEquals("Low Tom", pads.get(40).name);
    }

    @Test
    @DisplayName("stops filling pads before dropping the samples when another project comes to the front while the pads catch up")
    void stopsFillingPadsWhileThePadsCatchUp() {
        // The pad list starts at note 36, so the extension first moves it down to note 0.
        studio(m -> m.padScroll = 36);
        app.waitFor(obj("id", 1, "cmd", "insert", "track", 0, "at", "start", "kind", "bitwig", "device", DRUM_MACHINE));
        // The Drum Machine is selected and its pads start from note 0, and while the extension waits for the pads to
        // catch up, just before it drops the samples on them, someone brings their own project to the front.
        bitwig.switchDuring("pads", () -> Boolean.TRUE.equals(bitwig.cursorDevice.drum.v) && ((Number) bitwig.pads.scroll.v).intValue() == 0,
                            "My Song");
        List<Object> kit = list(obj("note", 36, "path", "C:\\Kit\\Kick.wav", "name", "Big Kick"), obj("note", 38, "path", "C:\\Kit\\Snare.wav", "name", "Big Snare"));
        assertJson(obj("id", 2, "ok", false, "error", elsewhere("My Song", "Night Drive")),
                   app.waitFor(obj("id", 2, "cmd", "pads", "track", 0, "device", 0, "pads", kit, "onlyIn", "Night Drive")));
        assertEquals(List.of(), bitwig.calls.stream().filter(c -> c.size() > 2 && String.valueOf(c.get(2)).startsWith("pad "))
                                            .collect(Collectors.toList()), "no sample dropped on a pad");
        assertNull(bitwig.state.tracks.get(0).devices.get(0).pads);

        bitwig.projectName.v = "Night Drive";
        assertJson(obj("filled", list(40), "replaced", list(), "skipped", list(), "failed", list(), "taken", list()),
                   app.waitFor(obj("id", 3, "cmd", "pads", "track", 0, "device", 0, "pads", list(obj("note", 40, "path", "C:\\Kit\\Tom.wav", "name", "Low Tom")),
                                   "onlyIn", "Night Drive")).get("result"));
        assertEquals("Low Tom", bitwig.state.tracks.get(0).devices.get(0).pads.get(40).name);
    }

    @Test
    @DisplayName("doesn't start a change that waited its turn once another project is in front")
    void doesntStartAQueuedChangeInAnotherProject() {
        studio();
        app.send(obj("id", 1, "cmd", "newtrack", "name", "One", "onlyIn", "Night Drive"));
        app.send(obj("id", 2, "cmd", "clip", "track", 0, "path", "C:\\MIDI\\Bass.mid", "onlyIn", "Night Drive"));
        app.send(obj("id", 3, "cmd", "pads", "track", 0, "device", 0, "pads", list(obj("note", 36, "path", "C:\\Kit\\Kick.wav")), "onlyIn", "Night Drive"));
        assertEquals(List.of(), app.take(), "they wait their turn");
        bitwig.clock.advance(10);
        bitwig.projectName.v = "My Song";
        bitwig.clock.advance(20000);
        List<Object> refused = new ArrayList<>();
        for (int id = 1; id <= 3; id++) refused.add(obj("id", id, "ok", false, "error", elsewhere("My Song", "Night Drive")));
        assertJson(refused, app.take());
        assertEquals(List.of("createInstrumentTrack"), bitwig.calls.stream().map(c -> c.get(0)).collect(Collectors.toList()),
                     "only what the first asked before the change");
        assertEquals(List.of(), bitwig.state.tracks.get(0).slots);

        bitwig.projectName.v = "Night Drive";
        assertJson(obj("slot", 0), app.waitFor(obj("id", 4, "cmd", "clip", "track", 0, "path", "C:\\MIDI\\Bass.mid", "onlyIn", "Night Drive")).get("result"));
    }

    @Test
    @DisplayName("stops a change part way when changes are turned off meanwhile")
    void stopsAChangeWhenChangesAreTurnedOff() {
        studio();
        app.send(obj("id", 1, "cmd", "newtrack", "name", "Lead", "color", "#ff8000"));
        app.send(obj("id", 2, "cmd", "newtrack", "name", "Pad"));   // waits its turn
        bitwig.clock.advance(10);
        bitwig.allowChanges(false);
        bitwig.clock.advance(20000);
        List<Map<String, Object>> replies = app.take();
        assertJson(list(list(1, false), list(2, false)), replies.stream().map(r -> list(r.get("id"), r.get("ok"))).collect(Collectors.toList()));
        for (Map<String, Object> reply : replies) assertMatches(CHANGES_OFF, error(reply));
        assertEquals(untouched(), looks());
        assertEquals(List.of("createInstrumentTrack"), bitwig.calls.stream().map(c -> c.get(0)).collect(Collectors.toList()));
    }

    // ---- Helpers ----

    private List<List<Object>> calls(String method) {
        return bitwig.calls.stream().filter(c -> c.get(0).equals(method)).collect(Collectors.toList());
    }

    private static List<Object> names(List<Object> items) {
        return items.stream().map(t -> at(t, "name")).collect(Collectors.toList());
    }

    private static List<String> deviceNames(TrackSpec track) {
        return track.devices.stream().map(d -> d.name).collect(Collectors.toList());
    }
}
