package org.musicorganiser.bitwig;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.musicorganiser.bitwig.FakeBitwig.device;
import static org.musicorganiser.bitwig.FakeBitwig.knob;
import static org.musicorganiser.bitwig.Messages.assertJson;
import static org.musicorganiser.bitwig.Messages.list;
import static org.musicorganiser.bitwig.Messages.obj;

import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.stream.Collectors;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;
import org.musicorganiser.bitwig.FakeBitwig.App;

/** The link's log (`<date>-link.jsonl` in the reports folder), and the folder kept tidy. */
class LinkLogTest {
    @TempDir
    Path reports;

    @Test
    @DisplayName("logs every command it handles but a hello that went well, with how long it took and how it went, and nothing of the project")
    void logsEveryCommand() throws IOException {
        FakeBitwig bitwig = FakeBitwig.start(LinkTest.song(), reports);
        App app = bitwig.connect();
        app.take();
        app.ask(obj("id", 1, "cmd", "hello"));
        app.ask(obj("id", 2, "cmd", "project"));
        app.ask("{not json");
        app.ask(obj("id", 4, "cmd", "explode"));
        app.ask(obj("id", 5, "cmd", "set", "param", "p", "value", 0.5));
        bitwig.allowChanges(true);
        app.waitFor(obj("id", 6, "cmd", "newtrack", "name", "Lead"));
        app.close();
        bitwig.link.exit();

        List<Map<String, Object>> lines = log();
        for (Map<String, Object> line : lines) assertEquals("2026-10-14T", ((String) line.get("time")).substring(0, 11), Json.write(line));
        List<Object> events = lines.stream().filter(l -> l.containsKey("event")).map(l -> l.get("event")).collect(Collectors.toList());
        assertEquals(List.of("start", "connected", "disconnected", "stop"), events);
        assertJson(obj("event", "start", "extension", MusicOrganiserExtensionDefinition.VERSION, "bitwig", "6.1.2", "api", 25, "port", 19791), without(lines.get(0), "time"));

        List<Object> commands = lines.stream().filter(l -> l.containsKey("cmd")).map(l -> list(l.get("cmd"), l.get("ok"), l.get("error")))
                                     .collect(Collectors.toList());
        assertJson(list(list("project", true, null), list(null, false, "The request isn't JSON: {not json"),
                        list("explode", false, "Unknown command: explode"), list("set", false, Link.CHANGES_OFF), list("newtrack", true, null)),
                   commands);
        // How long each took, in the stand-in's time: answers at once, and a new track once Bitwig has added, named and selected it.
        assertJson(list(0, 0, 0, 0, 300),
                   lines.stream().filter(l -> l.containsKey("cmd")).map(l -> l.get("ms")).collect(Collectors.toList()));
        for (Map<String, Object> line : lines) assertFalse(line.containsKey("result"), Json.write(line));

        String text = Files.readString(reports.resolve("2026-10-14-link.jsonl"), StandardCharsets.US_ASCII);
        for (String name : new String[] {"Night Drive", "Bass", "Serum", "Lead"}) assertFalse(text.contains(name), name);
    }

    @Test
    @DisplayName("answers nothing more once Bitwig stops it")
    void stopsWhenBitwigStopsIt() throws IOException {
        FakeBitwig bitwig = FakeBitwig.start(LinkTest.song(), reports);
        bitwig.cursorDevice.show(device("Serum"), knob("p", "Cutoff", 0, "0"));
        App app = bitwig.connect();
        app.take();
        app.send(obj("id", 1, "cmd", "device"));
        bitwig.link.exit();
        bitwig.clock.advance(3000);
        app.send(obj("id", 2, "cmd", "hello"));
        assertEquals(List.of(), app.take());
        assertEquals(List.of(), bitwig.errors);
        List<Map<String, Object>> lines = log();
        assertEquals("stop", lines.get(2).get("event"));
    }

    @Test
    @DisplayName("deletes its own reports from more than two weeks ago when it starts, and nothing else")
    void tidiesItsReports() throws IOException {
        for (String name : new String[] {
            "2026-09-29-link.jsonl", "2026-09-30-link.jsonl", "2026-09-01T120000.000Z-selfcheck.json", "2026-10-13T120000.000Z-selfcheck.json",
            "2026-01-01 notes.txt", "probe.jsonl"
        }) Files.writeString(reports.resolve(name), "x");
        FakeBitwig.start(LinkTest.song(), reports);
        assertEquals(List.of("2026-01-01 notes.txt", "2026-09-30-link.jsonl", "2026-10-13T120000.000Z-selfcheck.json", "2026-10-14-link.jsonl",
                             "probe.jsonl"),
                     SelfCheckTest.list(reports));
    }

    private List<Map<String, Object>> log() throws IOException {
        List<Map<String, Object>> lines = new ArrayList<>();
        for (String line : Files.readAllLines(reports.resolve("2026-10-14-link.jsonl"), StandardCharsets.US_ASCII))
            lines.add(SelfCheckTest.map(Json.parse(line)));
        return lines;
    }

    private static Map<String, Object> without(Map<String, Object> map, String key) {
        Map<String, Object> copy = new java.util.LinkedHashMap<>(map);
        copy.remove(key);
        return copy;
    }
}
