package org.musicorganiser.bitwig;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assumptions.assumeTrue;
import static org.musicorganiser.bitwig.Messages.list;
import static org.musicorganiser.bitwig.Messages.obj;

import java.io.IOException;
import java.io.OutputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.TimeUnit;
import java.util.stream.Collectors;
import java.util.stream.Stream;
import org.junit.jupiter.api.DynamicTest;
import org.junit.jupiter.api.TestFactory;
import org.junit.jupiter.api.io.TempDir;
import org.musicorganiser.bitwig.FakeBitwig.App;
import org.musicorganiser.bitwig.FakeBitwig.DeviceSpec;
import org.musicorganiser.bitwig.FakeBitwig.Knob;
import org.musicorganiser.bitwig.FakeBitwig.Model;
import org.musicorganiser.bitwig.FakeBitwig.TrackSpec;

/**
 * Sends the same requests (plenty of them odd) to the JavaScript script, running in its own stand-in Bitwig with
 * Node.js (src/test/js/run-script.js), and to the extension in FakeBitwig, and checks every message that comes
 * back is the same, apart from what link version 3 adds on purpose. The script is the reference: while both are
 * in use, this keeps them from drifting apart, and it checks FakeBitwig behaves as the script's stand-in does. The
 * commands only the extension has (clicked, recorder, selftest, notes, transport, stoptracks and the arrange experiment) aren't sent: the script
 * answers each with "Unknown command", and NewerApiTest, SelfCheckTest and ArrangerTest check them.
 *
 * Without Node.js on the PATH it's skipped, except on CI, where the workflow installs it.
 */
class ParityTest {
    static final String DRUM_MACHINE = LinkTest.DRUM_MACHINE;

    @TempDir
    Path reports;

    record Run(String name, Map<String, Object> model, List<Map<String, Object>> steps) {}

    @TestFactory
    Stream<DynamicTest> answersAsTheScriptDoes() throws Exception {
        Path project = Paths.get(System.getProperty("basedir", "")).toAbsolutePath();
        Path stand = project.resolve("../bitwig/test/fake-bitwig.js").normalize();
        Path runner = project.resolve("src/test/js/run-script.js");
        assumeTrue(Files.isRegularFile(stand), "The JavaScript script's stand-in isn't at " + stand);
        boolean node = nodeRuns();
        // CI always compares them (the workflow installs Node.js); on a computer without it, this is skipped.
        if (!node && System.getenv("CI") != null) throw new AssertionError("Node.js is needed to compare the extension with the script.");
        assumeTrue(node, "Node.js isn't installed, so the extension isn't compared with the script.");

        List<Run> runs = runs();
        List<Object> script = scriptAnswers(runner, stand, runs);
        List<DynamicTest> tests = new ArrayList<>();
        for (int r = 0; r < runs.size(); r++) {
            Run run = runs.get(r);
            List<?> expected = (List<?>) script.get(r);
            tests.add(DynamicTest.dynamicTest(run.name(), () -> {
                List<List<Map<String, Object>>> actual = extensionAnswers(run);
                for (int s = 0; s < expected.size(); s++) {
                    String step = s == 0 ? "connecting" : "step " + s + ", " + Json.write(run.steps().get(s - 1));
                    assertEquals(Json.write(expected.get(s)), Json.write(normalise(actual.get(s))), step);
                }
            }));
        }
        return tests.stream();
    }

    /** What the extension says, in FakeBitwig, for the same steps. */
    private List<List<Map<String, Object>>> extensionAnswers(Run run) throws IOException {
        FakeBitwig bitwig = FakeBitwig.start(model(run.model()), Files.createDirectories(reports.resolve(run.name())));
        App app = bitwig.connect();
        List<List<Map<String, Object>>> out = new ArrayList<>();
        out.add(app.take());
        for (Map<String, Object> step : run.steps()) {
            if (step.containsKey("send")) app.send(Json.write(step.get("send")));
            else if (step.containsKey("raw")) app.send(step.get("raw"));
            else if (step.containsKey("advance")) bitwig.clock.advance(((Number) step.get("advance")).longValue());
            else if (step.containsKey("allowChanges")) bitwig.allowChanges((Boolean) step.get("allowChanges"));
            else if (step.containsKey("show")) bitwig.cursorDevice.show(device(map(Messages.at(step, "show", "device"))),
                                                                         knobs((List<?>) Messages.at(step, "show", "knobs")).toArray(new Knob[0]));
            else if (step.containsKey("close")) app.close();
            else if (step.containsKey("switchDuring")) bitwig.switchDuring((String) step.get("switchDuring"), (String) step.get("project"));
            else if (step.containsKey("project")) bitwig.projectName.v = step.get("project");
            else throw new IllegalArgumentException("Unknown step " + Json.write(step));
            out.add(app.take());
        }
        return out;
    }

    /** What the script says: run-script.js runs every run in one go. */
    private List<Object> scriptAnswers(Path runner, Path stand, List<Run> runs) throws Exception {
        List<Object> input = new ArrayList<>();
        for (Run run : runs) input.add(obj("model", run.model(), "steps", run.steps()));
        Path errors = Files.createTempFile(reports, "node", ".txt");
        Process node = new ProcessBuilder("node", runner.toString(), stand.toString()).redirectError(errors.toFile()).start();
        try (OutputStream stdin = node.getOutputStream()) {
            stdin.write(Json.write(input).getBytes(StandardCharsets.UTF_8));
        }
        byte[] output = node.getInputStream().readAllBytes();
        if (!node.waitFor(60, TimeUnit.SECONDS) || node.exitValue() != 0)
            throw new AssertionError("Node.js couldn't run the script: " + Files.readString(errors));
        return Messages.listAt(Json.parse(new String(output, StandardCharsets.UTF_8)));
    }

    private static boolean nodeRuns() {
        try {
            Process node = new ProcessBuilder("node", "--version").redirectErrorStream(true).redirectOutput(ProcessBuilder.Redirect.DISCARD).start();
            return node.waitFor(30, TimeUnit.SECONDS) && node.exitValue() == 0;
        } catch (IOException e) {
            return false;
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            return false;
        }
    }

    /**
     * The extension's answers as the script would give them: link version 3 adds to hello (`link` is 3, with
     * `runtime`, `extension` and `loadedFrom`) and to device (the remote controls `page` and its `controls`), and nothing else.
     */
    static Object normalise(Object value) {
        if (value instanceof Map) {
            Map<String, Object> m = new LinkedHashMap<>();
            for (Map.Entry<?, ?> e : ((Map<?, ?>) value).entrySet()) m.put((String) e.getKey(), normalise(e.getValue()));
            if (m.containsKey("link") && m.containsKey("runtime")) {
                m.put("link", 2.0);
                m.remove("runtime");
                m.remove("extension");
                m.remove("loadedFrom");
                m.remove("loopback");
            }
            if (m.containsKey("params") && m.containsKey("drumPads")) {
                m.remove("page");
                m.remove("controls");
            }
            return m;
        }
        if (value instanceof List) return ((List<?>) value).stream().map(ParityTest::normalise).collect(Collectors.toList());
        return value;
    }

    // ---- What's sent ----

    static Map<String, Object> send(Object request) {
        return obj("send", request);
    }

    static Map<String, Object> raw(String text) {
        return obj("raw", text);
    }

    static Map<String, Object> advance(long ms) {
        return obj("advance", ms);
    }

    static Map<String, Object> allowChanges() {
        return obj("allowChanges", true);
    }

    static Map<String, Object> show(Map<String, Object> device, Object... knobs) {
        return obj("show", obj("device", device, "knobs", list(knobs)));
    }

    /** Another project comes to the front now. */
    static Map<String, Object> project(String name) {
        return obj("project", name);
    }

    /** Another project comes to the front once the next change asked for with `cmd` first waits for Bitwig. */
    static Map<String, Object> switchDuring(String cmd, String name) {
        return obj("switchDuring", cmd, "project", name);
    }

    /** A request for each value of one field, the others as given: a field's odd values, in one go. */
    static List<Map<String, Object>> each(Map<String, Object> request, String field, Object... values) {
        List<Map<String, Object>> steps = new ArrayList<>();
        for (Object v : values) {
            Map<String, Object> r = new LinkedHashMap<>(request);
            if (v == MISSING) r.remove(field);
            else r.put(field, v);
            steps.add(send(r));
        }
        return steps;
    }

    /** A field left out of a request. */
    static final Object MISSING = new Object();

    static Map<String, Object> song() {
        return obj(
            "project", "Night Drive", "tempo", 124, "timeSignature", "7/8", "selectedTrack", "Bass",
            "tracks", list(
                obj("name", "Drums", "type", "Group", "group", true, "devices", list()),
                obj("name", "Bass", "type", "Instrument", "colour", list(1, 0.5, 0), "volume", list(0.6, "-6.0 dB"), "devices", list(
                    obj("name", "Serum", "preset", "Reese", "plugin", true, "kind", "note_to_audio"),
                    obj("name", "EQ+", "plugin", false, "enabled", false))),
                obj("name", "Master", "type", "Master", "devices", list(obj("name", "Peak Limiter")))),
            "actions", list(
                obj("id", "Save", "name", "Save", "category", "File"),
                obj("id", "toggle_metronome", "name", "Toggle Metronome", "category", "Transport", "menu", "Metronome")));
    }

    static Map<String, Object> studio() {
        return obj(
            "project", "Night Drive", "drumMachine", DRUM_MACHINE, "selectedTrack", "Bass", "scenes", 2, "padScroll", 36, "presets", true,
            "tracks", list(
                obj("name", "Bass", "type", "Instrument", "devices", list(obj("name", "Serum", "plugin", true, "kind", "note_to_audio")),
                    "slots", list(obj("file", "old.mid"))),
                obj("name", "Reverb", "type", "Effect", "devices", list(obj("name", "Reverb"))),
                obj("name", "Master", "type", "Master", "devices", list())),
            "files", obj("C:\\Presets\\Lead.bwpreset", obj("name", "Serum", "preset", "Lead", "plugin", true, "kind", "note_to_audio")),
            "plugins", obj(
                "VST3:805ECA49563E43EA8119369F0ED6096D", obj("name", "Serum", "plugin", true, "kind", "note_to_audio"),
                "CLAP:com.u-he.Diva", obj("name", "Diva", "plugin", true, "kind", "note_to_audio"),
                "VST2:1299137074", obj("name", "Massive", "plugin", true, "kind", "note_to_audio")),
            "knobs", obj("Serum", list(
                obj("id", "CONTENTS/PID0", "name", "Master Volume", "value", 0.5, "display", "50%"),
                obj("id", "CONTENTS/PID2", "name", "Osc B Level", "value", 0.5, "display", "50%", "locked", true),
                obj("id", "CONTENTS/PID3e8", "name", "Filter Cutoff", "value", 0.5))));
    }

    static List<Run> runs() {
        List<Run> runs = new ArrayList<>();

        List<Map<String, Object>> odd = new ArrayList<>();
        for (String text : new String[] {
            "{not json", "null", "0", "false", "true", "\"\"", "\"text\"", "[1,2]", "[]", "5", "{}", "{\"a\": 1,}", "",
            "{\"id\": 1e400, \"cmd\": \"x\"}", "  {\"id\": 9, \"cmd\": \"hello\"}\n", "{\"__proto__\": {\"cmd\": \"hello\"}, \"id\": 3}",
            "{\"id\": 4, \"cmd\": \"hello\", \"cmd\": \"explode\"}", "{\"name\": \"" + "Café ☕ ".repeat(20)
        }) odd.add(raw(text));
        odd.addAll(each(obj("id", 2, "cmd", "explode"), "cmd", "explode", null, 5, -0.0, 1e21, 0.1, list("a", "b"), list(), obj(), true,
                        "toString", "hasOwnProperty", "__proto__", "constructor", "HELLO", " hello", "Café ☕ 𝄞", MISSING));
        odd.addAll(each(obj("cmd", "hello"), "id", obj("a", list(1, null)), "x", 1.5, -0.0, null, true, list(), MISSING));
        // Changes are off, so each of these says so, whatever else is wrong with it.
        for (String cmd : new String[] {"set", "insert", "action", "newtrack", "clip", "pads", "transpose"})
            odd.addAll(each(obj("id", cmd, "cmd", cmd), "track", MISSING, 0, 99, "x"));
        odd.add(send(obj("id", 5, "cmd", "project")));
        odd.add(send(obj("id", 6, "cmd", "actions")));
        odd.add(send(obj("id", 7, "cmd", "device")));
        runs.add(new Run("requests it can't follow", song(), odd));

        List<Map<String, Object>> reading = new ArrayList<>();
        reading.add(allowChanges());
        reading.addAll(each(obj("id", 1, "cmd", "select"), "track", MISSING, null, -1, 0.5, "1", 1e300, 3, true, obj(), list(1), 128));
        reading.addAll(each(obj("id", 2, "cmd", "select", "track", 1), "device", "0", 1.5, 5, -1, 16, true, null));
        reading.add(send(obj("id", 3, "cmd", "select", "track", 0, "device", 0)));
        reading.add(send(obj("id", 3, "cmd", "select", "track", 1, "device", 1)));
        reading.addAll(each(obj("id", 4, "cmd", "set", "param", "p", "value", 0.5), "param", "p", MISSING));
        reading.add(advance(100));
        reading.add(send(obj("id", 5, "cmd", "device")));
        reading.add(advance(100));
        reading.add(show(obj("name", "Serum", "preset", "Reese", "plugin", true, "kind", "note_to_audio"),
                         obj("id", "CONTENTS/PID3e8", "name", "Cutoff", "value", 0.25, "display", "440 Hz"),
                         obj("id", "CONTENTS/PID3e9", "name", "Silent", "value", 0)));
        reading.addAll(each(obj("id", 6, "cmd", "set", "value", 0.5), "param", MISSING, null, 5, list("CONTENTS/PID3e8"), "nope", "CONTENTS/PID3e8"));
        reading.addAll(each(obj("id", 7, "cmd", "set", "param", "CONTENTS/PID3e8"), "value",
                            MISSING, null, true, "0.5", -0.1, 1.5, list(0.5), 0, 1, -0.0, 0.123456789, 1e-9));
        reading.add(send(obj("id", 8, "cmd", "device")));
        reading.add(send(obj("id", 9, "cmd", "device")));
        reading.add(advance(1990));
        reading.add(advance(100));
        // Straight away is the one route both run an action by (the script refuses the others). Without one, the script runs
        // it straight away and the extension (from 3.8) from Bitwig's flush, so these name it.
        reading.addAll(each(obj("id", 10, "cmd", "action", "via", "direct"), "action", MISSING, null, 5, obj(), "Format C:", "save", "Save",
                            "toggle_metronome"));
        reading.add(send(obj("id", 11, "cmd", "actions")));
        reading.add(send(obj("id", 12, "cmd", "project")));
        reading.add(send(obj("id", 13, "cmd", "hello")));
        reading.add(send(obj("id", 14, "cmd", "device")));
        reading.add(obj("close", true));
        reading.add(advance(3000));
        runs.add(new Run("reading, selecting and setting", song(), reading));

        List<Map<String, Object>> sending = new ArrayList<>();
        sending.add(allowChanges());
        Map<String, Object> insert = obj("id", 1, "cmd", "insert", "kind", "vst3", "plugin", "805ECA49563E43EA8119369F0ED6096D");
        sending.addAll(each(insert, "kind", MISSING, null, 5, "VST3", "aax", ""));
        for (String kind : new String[] {"vst3", "clap"})
            sending.addAll(each(obj("id", 2, "cmd", "insert", "kind", kind), "plugin", MISSING, null, "", 5, list("x")));
        sending.addAll(each(obj("id", 3, "cmd", "insert", "kind", "vst2"), "plugin", MISSING, null, "MoB2", true, "1299137074"));
        sending.addAll(each(obj("id", 4, "cmd", "insert", "kind", "bitwig"), "device", MISSING, null, "", 5));
        sending.addAll(each(obj("id", 5, "cmd", "insert", "kind", "file"), "path", MISSING, null, "", 5));
        sending.addAll(each(obj("id", 6, "cmd", "insert", "kind", "file", "path", "x.wav", "track", 0), "at", "middle", "", "START", 0));
        sending.add(send(obj("id", 7, "cmd", "insert", "kind", "file", "path", "x.wav", "at", "start")));
        sending.addAll(each(obj("id", 8, "cmd", "insert", "kind", "file", "path", "x.wav"), "track", 9, -1, "0", 0.5, true, 128));
        // On the selected track: answers at once.
        sending.add(send(obj("id", 9, "cmd", "insert", "kind", "vst3", "plugin", "805ECA49563E43EA8119369F0ED6096D")));
        sending.add(send(obj("id", 10, "cmd", "insert", "kind", "vst2", "plugin", 1299137074)));
        sending.add(advance(1000));
        // On a given track: waits to say what Bitwig added.
        for (Map<String, Object> request : List.of(
                obj("id", 11, "cmd", "insert", "track", 0, "kind", "file", "path", "C:\\Samples\\Pluck.wav"),
                obj("id", 12, "cmd", "insert", "track", 0, "at", "start", "kind", "bitwig", "device", FakeBitwig.NOTE_TRANSPOSE),
                obj("id", 13, "cmd", "insert", "track", 2, "kind", "file", "path", "C:\\Presets\\Lead.bwpreset"),
                obj("id", 14, "cmd", "insert", "track", 2, "at", "end", "kind", "file", "path", "C:\\Presets\\Serum - Pluck.vstpreset"),
                obj("id", 15, "cmd", "insert", "track", 1, "at", null, "kind", "clap", "plugin", "com.u-he.Diva"),
                obj("id", 16, "cmd", "insert", "track", 0, "kind", "vst3", "plugin", "00000000000000000000000000000000"),
                obj("id", 17, "cmd", "insert", "track", 0, "kind", "file", "path", "C:\\notes.txt"))) {
            sending.add(send(request));
            sending.add(advance(10000));
        }
        sending.addAll(each(obj("id", 18, "cmd", "newtrack"), "name", 7, true, obj(), list()));
        sending.addAll(each(obj("id", 19, "cmd", "newtrack"), "color", "orange", "#FFF", 5, "#ff800g", "ff8000", list()));
        for (Map<String, Object> request : List.of(
                obj("id", 20, "cmd", "newtrack", "name", "Lead", "color", "#FF8000"),
                obj("id", 21, "cmd", "newtrack", "name", "", "color", null),
                obj("id", 22, "cmd", "newtrack", "name", "Café ☕ 𝄞"))) {
            sending.add(send(request));
            sending.add(advance(10000));
        }
        sending.add(send(obj("id", 23, "cmd", "project")));
        // One at a time, in the order asked; reading doesn't wait.
        sending.add(send(obj("id", 24, "cmd", "newtrack", "name", "One")));
        sending.add(send(obj("id", 25, "cmd", "newtrack", "name", "Two")));
        sending.add(send(obj("id", 26, "cmd", "project")));
        sending.add(advance(20000));
        runs.add(new Run("inserting and adding tracks", studio(), sending));

        List<Map<String, Object>> filling = new ArrayList<>();
        filling.add(allowChanges());
        filling.addAll(each(obj("id", 1, "cmd", "clip", "path", "C:\\MIDI\\x.mid"), "track", MISSING, null, 9, "0"));
        filling.addAll(each(obj("id", 2, "cmd", "clip", "track", 0), "path", MISSING, "", 5));
        filling.addAll(each(obj("id", 3, "cmd", "clip", "track", 0, "path", "C:\\MIDI\\x.mid"), "slot", 8, -1, 1.5, "0", 0));
        for (Map<String, Object> request : List.of(
                obj("id", 4, "cmd", "clip", "track", 0, "path", "C:\\MIDI\\Bass.mid"),
                obj("id", 5, "cmd", "clip", "track", 0, "path", "C:\\MIDI\\Bass 2.mid"),
                obj("id", 6, "cmd", "clip", "track", 0, "slot", 5, "path", "C:\\MIDI\\Bass 5.mid"),
                obj("id", 7, "cmd", "clip", "track", 1, "path", "C:\\notes.txt"),
                obj("id", 8, "cmd", "clip", "track", 1, "slot", null, "path", "C:\\MIDI\\Verb.MIDI"))) {
            filling.add(send(request));
            filling.add(advance(10000));
        }
        Map<String, Object> kick = obj("note", 36, "path", "C:\\Kit\\Kick.wav");
        filling.addAll(each(obj("id", 9, "cmd", "pads"), "pads", MISSING, null, "x", list(), obj(), list((Object) null), list(5),
                            list(obj("note", -1, "path", "a.wav")), list(obj("note", 128, "path", "a.wav")), list(obj("note", "36", "path", "a.wav")),
                            list(obj("note", 36.5, "path", "a.wav")), list(obj("note", 36)), list(obj("note", 36, "path", 5)),
                            list(obj("note", 36, "path", "a.wav", "name", 5)), list(kick, kick)));
        filling.add(send(obj("id", 10, "cmd", "pads", "pads", list(kick))));   // nothing selected
        filling.addAll(each(obj("id", 11, "cmd", "pads", "pads", list(kick)), "track", 9, "0"));
        filling.addAll(each(obj("id", 12, "cmd", "pads", "pads", list(kick), "track", 0), "device", 3, "0", -1));
        for (Map<String, Object> request : List.of(
                obj("id", 13, "cmd", "pads", "track", 0, "pads", list(kick)),
                obj("id", 14, "cmd", "insert", "track", 0, "at", "start", "kind", "bitwig", "device", DRUM_MACHINE),
                obj("id", 15, "cmd", "pads", "track", 0, "device", 0, "pads", list(
                    obj("note", 36, "path", "C:\\Kit\\Kick.wav", "name", "Kick"), obj("note", 38, "path", "C:\\Kit\\Snare.WAV", "name", ""),
                    obj("note", 42, "path", "C:\\Kit\\notes.txt", "name", "Hat"), obj("note", 0, "path", "C:\\Kit\\Low.flac"))),
                obj("id", 16, "cmd", "pads", "track", 0, "pads", list(obj("note", 36, "path", "C:\\Kit\\Kick 2.wav"), obj("note", 37, "path", "C:\\Kit\\Rim.wav"))),
                obj("id", 17, "cmd", "select", "track", 0, "device", 1),
                obj("id", 18, "cmd", "pads", "pads", list(obj("note", 40, "path", "C:\\Kit\\Tom.wav"))),
                obj("id", 19, "cmd", "select", "track", 0, "device", 0),
                obj("id", 20, "cmd", "pads", "pads", list(obj("note", 40, "path", "C:\\Kit\\Tom.wav"))),
                obj("id", 21, "cmd", "device"))) {
            filling.add(send(request));
            filling.add(advance(10000));
        }
        filling.addAll(each(obj("id", 22, "cmd", "transpose", "track", 0, "semitones", 2), "device", MISSING, 2, "0", -1, 0.5));
        filling.addAll(each(obj("id", 23, "cmd", "transpose", "device", 0, "semitones", 2), "track", MISSING, 9));
        filling.addAll(each(obj("id", 24, "cmd", "transpose", "track", 0, "device", 0), "semitones", MISSING, null, "up", true, list(2)));
        for (Map<String, Object> request : List.of(
                obj("id", 25, "cmd", "transpose", "track", 0, "device", 0, "semitones", 2),
                obj("id", 26, "cmd", "insert", "track", 2, "at", "start", "kind", "file", "path", "C:\\Samples\\Pluck.aif"),
                obj("id", 27, "cmd", "transpose", "track", 2, "device", 0, "semitones", -3.25),
                obj("id", 28, "cmd", "transpose", "track", 2, "device", 0, "semitones", -100),
                obj("id", 29, "cmd", "insert", "track", 2, "at", "start", "kind", "bitwig", "device", FakeBitwig.NOTE_TRANSPOSE),
                obj("id", 30, "cmd", "transpose", "track", 2, "device", 0, "semitones", 7.4),
                obj("id", 31, "cmd", "transpose", "track", 2, "device", 1, "semitones", 0),
                obj("id", 32, "cmd", "project"))) {
            filling.add(send(request));
            filling.add(advance(10000));
        }
        runs.add(new Run("clips, pads and pitch", studio(), filling));

        // Only in the project a request names (onlyIn): checked as it arrives, as a change starts and before each step
        // of a change after a wait.
        List<Map<String, Object>> only = new ArrayList<>();
        // Changes are off: reads go ahead only in the project named, and a change says what's wrong first.
        only.addAll(each(obj("id", 1, "cmd", "hello"), "onlyIn", "Night Drive", "night drive", "Night Drive ", "", "Café ☕ 𝄞", null,
                         5, 0, -0.0, true, false, list("Night Drive"), list(), obj(), MISSING));
        only.addAll(each(obj("id", 2, "cmd", "newtrack", "name", "Lead"), "onlyIn", "Night Drive", "Elsewhere", 5, null));
        only.add(allowChanges());
        for (String cmd : new String[] {"set", "insert", "action", "newtrack", "clip", "pads", "transpose", "select", "project", "device",
                                        "actions", "hello", "explode"})
            only.addAll(each(obj("id", cmd, "cmd", cmd), "onlyIn", "Elsewhere", 5));
        // In the project named, a change goes ahead as it does without onlyIn.
        for (Map<String, Object> request : List.of(
                obj("id", 3, "cmd", "newtrack", "name", "Lead", "color", "#ff8000", "onlyIn", "Night Drive"),
                obj("id", 4, "cmd", "insert", "track", 1, "kind", "bitwig", "device", DRUM_MACHINE, "onlyIn", "Night Drive"),
                obj("id", 5, "cmd", "pads", "track", 1, "device", 0, "pads", list(obj("note", 36, "path", "C:\\Kit\\Kick.wav", "name", "Kick")),
                    "onlyIn", "Night Drive"),
                obj("id", 6, "cmd", "select", "track", 0, "onlyIn", "Night Drive"),
                obj("id", 7, "cmd", "clip", "track", 0, "path", "C:\\MIDI\\Bass.mid", "onlyIn", "Night Drive"))) {
            only.add(send(request));
            only.add(advance(10000));
        }
        // Another project comes to the front just after a new track is asked for: that change stops, and the next one,
        // sent after, is refused as it arrives.
        only.add(switchDuring("newtrack", "My Song"));
        only.add(send(obj("id", 8, "cmd", "newtrack", "name", "Two", "color", "#00ff00", "onlyIn", "Night Drive")));
        only.add(send(obj("id", 9, "cmd", "clip", "track", 0, "path", "C:\\MIDI\\Bass 2.mid", "onlyIn", "Night Drive")));
        only.add(advance(10000));
        // Changes that waited their turn don't start once it's in front, but one that names no project does.
        only.add(project("Night Drive"));
        only.add(send(obj("id", 10, "cmd", "newtrack", "name", "Three", "onlyIn", "Night Drive")));
        only.add(send(obj("id", 11, "cmd", "clip", "track", 0, "path", "C:\\MIDI\\Bass 3.mid", "onlyIn", "Night Drive")));
        only.add(send(obj("id", 12, "cmd", "transpose", "track", 1, "device", 0, "semitones", 2, "onlyIn", "Night Drive")));
        only.add(send(obj("id", 13, "cmd", "clip", "track", 0, "path", "C:\\MIDI\\Bass 4.mid")));
        only.add(advance(10));
        only.add(project("My Song"));
        only.add(advance(10000));
        // It comes to the front at each step of a new track and of filling pads, until the change is done.
        for (int ms = 40; ms <= 340; ms += 50) {
            only.add(project("Night Drive"));
            only.add(send(obj("id", 14, "cmd", "newtrack", "name", "Track " + ms, "color", "#0000ff", "onlyIn", "Night Drive")));
            only.add(advance(ms));
            only.add(project("My Song"));
            only.add(advance(10000));
        }
        for (int ms = 40; ms <= 440; ms += 50) {
            only.add(project("Night Drive"));
            only.add(send(obj("id", 15, "cmd", "pads", "track", 1, "device", 0,
                              "pads", list(obj("note", 40 + ms / 50, "path", "C:\\Kit\\Tom " + ms + ".wav", "name", "Tom " + ms)), "onlyIn", "Night Drive")));
            only.add(advance(ms));
            only.add(project("My Song"));
            only.add(advance(10000));
        }
        // Changes are turned off part way through one, with another waiting its turn.
        only.add(project("Night Drive"));
        only.add(send(obj("id", 16, "cmd", "newtrack", "name", "Four", "onlyIn", "Night Drive")));
        only.add(send(obj("id", 17, "cmd", "newtrack", "name", "Five")));
        only.add(advance(10));
        only.add(obj("allowChanges", false));
        only.add(advance(10000));
        only.add(allowChanges());
        only.add(send(obj("id", 18, "cmd", "project")));
        runs.add(new Run("only in the project asked for", studio(), only));

        // Bitwig 6.1.3 doesn't say at first which port it gave the link; the app connecting shows it's 19791.
        Map<String, Object> unsaid = song();
        unsaid.put("portUnknown", 1000);
        runs.add(new Run("Bitwig doesn't say which port", unsaid, List.of(
            advance(2000), send(obj("id", 1, "cmd", "hello")))));

        // Solomon's default Drum Machine has an empty Sampler on every pad.
        Map<String, Object> preset = studio();
        preset.put("drumPads", list(obj("name", "Sampler", "id", Link.SAMPLER, "kind", "note_to_audio"), obj("name", "Compressor")));
        List<Map<String, Object>> kit = new ArrayList<>(List.of(allowChanges()));
        for (Map<String, Object> request : List.of(
                obj("id", 1, "cmd", "insert", "track", 0, "at", "start", "kind", "bitwig", "device", DRUM_MACHINE),
                obj("id", 2, "cmd", "pads", "track", 0, "device", 0, "pads", list(
                    obj("note", 36, "path", "C:\\Kit\\Kick.wav", "name", "Kick"), obj("note", 38, "path", "C:\\Kit\\notes.txt"))),
                obj("id", 3, "cmd", "pads", "track", 0, "device", 0, "pads", list(
                    obj("note", 36, "path", "C:\\Kit\\Kick 2.wav"), obj("note", 37, "path", "C:\\Kit\\Rim.wav"))),
                obj("id", 4, "cmd", "project"))) {
            kit.add(send(request));
            kit.add(advance(10000));
        }
        runs.add(new Run("a default Drum Machine with an empty Sampler on each pad", preset, kit));

        Map<String, Object> elsewhere = song();
        elsewhere.put("portInUse", 50123);
        elsewhere.put("modified", true);
        elsewhere.remove("selectedTrack");
        runs.add(new Run("another port, nothing selected", elsewhere, List.of(
            send(obj("id", 1, "cmd", "hello")), allowChanges(), send(obj("id", 2, "cmd", "hello")),
            send(obj("id", 3, "cmd", "device")),
            send(obj("id", 4, "cmd", "insert", "kind", "file", "path", "x.wav")),
            send(obj("id", 5, "cmd", "pads", "pads", list(obj("note", 36, "path", "a.wav")))))));
        return runs;
    }

    // ---- The script's model, as FakeBitwig's ----

    /** A project in fake-bitwig.js's terms, as a FakeBitwig model. */
    static Model model(Map<String, Object> js) {
        Model m = new Model();
        if (js.containsKey("project")) m.project = (String) js.get("project");
        if (js.containsKey("tempo")) m.tempo = number(js.get("tempo"));
        if (js.containsKey("timeSignature")) m.timeSignature = (String) js.get("timeSignature");
        m.modified = Boolean.TRUE.equals(js.get("modified"));
        m.selectedTrack = (String) js.get("selectedTrack");
        for (Object t : items(js.get("tracks"))) m.tracks.add(track(map(t)));
        for (Object a : items(js.get("actions"))) {
            Map<String, Object> action = map(a);
            m.actions.add(new FakeBitwig.ActionSpec((String) action.get("id"), (String) action.get("name"), (String) action.get("category"),
                                                    (String) action.get("menu")));
        }
        for (Map.Entry<String, Object> e : map(js.getOrDefault("files", Map.of())).entrySet()) m.files.put(e.getKey(), device(map(e.getValue())));
        m.presets = Boolean.TRUE.equals(js.get("presets"));
        for (Map.Entry<String, Object> e : map(js.getOrDefault("plugins", Map.of())).entrySet()) m.plugins.put(e.getKey(), device(map(e.getValue())));
        for (Map.Entry<String, Object> e : map(js.getOrDefault("knobs", Map.of())).entrySet()) m.knobs.put(e.getKey(), knobs(items(e.getValue())));
        m.drumMachine = (String) js.get("drumMachine");
        if (js.containsKey("scenes")) m.scenes = (int) number(js.get("scenes"));
        if (js.containsKey("padScroll")) m.padScroll = (int) number(js.get("padScroll"));
        m.browserOnNewTrack = Boolean.TRUE.equals(js.get("browserOnNewTrack"));
        if (js.containsKey("portInUse")) m.portInUse = (int) number(js.get("portInUse"));
        if (js.containsKey("portUnknown")) m.portUnknown = (int) number(js.get("portUnknown"));
        if (js.containsKey("drumPads")) m.drumPads = items(js.get("drumPads")).stream().map(d -> device(map(d))).collect(Collectors.toList());
        return m;
    }

    private static TrackSpec track(Map<String, Object> js) {
        TrackSpec t = new TrackSpec((String) js.get("name"), (String) js.get("type"));
        t.group = Boolean.TRUE.equals(js.get("group"));
        t.mute = Boolean.TRUE.equals(js.get("mute"));
        if (js.get("colour") != null) {
            List<Object> c = items(js.get("colour"));
            t.colour((float) number(c.get(0)), (float) number(c.get(1)), (float) number(c.get(2)));
        }
        if (js.get("volume") != null) t.volume(number(items(js.get("volume")).get(0)), (String) items(js.get("volume")).get(1));
        for (Object d : items(js.get("devices"))) t.devices.add(device(map(d)));
        for (Object slot : items(js.get("slots"))) t.slots.add(slot == null ? null : (String) map(slot).get("file"));
        return t;
    }

    private static DeviceSpec device(Map<String, Object> js) {
        DeviceSpec d = new DeviceSpec((String) js.get("name"));
        d.preset = (String) js.get("preset");
        d.plugin = Boolean.TRUE.equals(js.get("plugin"));
        d.kind = (String) js.get("kind");
        d.enabled = !Boolean.FALSE.equals(js.get("enabled"));
        d.id = (String) js.get("id");
        d.drum = Boolean.TRUE.equals(js.get("drum"));
        if (js.get("knobs") != null) d.knobs = knobs(items(js.get("knobs")));
        return d;
    }

    private static List<Knob> knobs(List<?> js) {
        List<Knob> knobs = new ArrayList<>();
        for (Object k : js) {
            Map<String, Object> knob = map(k);
            Knob made = new Knob((String) knob.get("id"), (String) knob.get("name"), number(knob.get("value")), (String) knob.get("display"));
            made.locked = Boolean.TRUE.equals(knob.get("locked"));
            knobs.add(made);
        }
        return knobs;
    }

    private static double number(Object value) {
        return ((Number) value).doubleValue();
    }

    private static List<Object> items(Object value) {
        return value == null ? List.of() : Messages.listAt(value);
    }

    private static Map<String, Object> map(Object value) {
        return SelfCheckTest.map(value);
    }
}
