package org.musicorganiser.bitwig;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.musicorganiser.bitwig.Messages.assertJson;
import static org.musicorganiser.bitwig.Messages.assertMatches;
import static org.musicorganiser.bitwig.Messages.at;
import static org.musicorganiser.bitwig.Messages.error;
import static org.musicorganiser.bitwig.Messages.listAt;
import static org.musicorganiser.bitwig.Messages.obj;

import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.stream.Collectors;
import java.util.stream.Stream;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;
import org.musicorganiser.bitwig.FakeBitwig.App;
import org.musicorganiser.bitwig.FakeBitwig.Model;

/** The self-check: what it checks, the file it writes, and how it reports a Bitwig that's missing a part. */
class SelfCheckTest {
    /** Every part of Bitwig's API the link uses or offers, in the order the self-check lists them. */
    static final List<String> CHECKS = List.of(
        "API version", "application", "project", "transport", "following", "cursor track", "cursor device", "drum pads", "track bank",
        "Sampler and Note Transpose pitch", "track IDs", "popup browser", "settings", "remote controls", "last clicked parameter",
        "parameter steps, origins and automation", "master recorder", "arranger", "clip notes", "files as tracks", "recording into the arranger", "command routes", "remote connection", "actions",
        "reports folder");

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
    @DisplayName("checks every part of Bitwig's API it uses, and writes what it found to the reports folder")
    void checksEveryPart() throws IOException {
        start(LinkTest.song());
        bitwig.allowChanges(true);
        Map<String, Object> result = map(app.ask(obj("id", 1, "cmd", "selftest")).get("result"));

        assertEquals(CHECKS, names(result));
        for (Object check : listAt(result, "checks")) {
            assertEquals(true, at(check, "ok"), Json.write(check));
            assertTrue(at(check, "detail") instanceof String && !((String) at(check, "detail")).isEmpty(), Json.write(check));
            assertTrue((Double) at(check, "ms") >= 0, Json.write(check));
        }
        assertEquals("124 BPM in 7/8, stopped.", detail(result, "transport"));
        assertEquals("Project \"Night Drive\", audio engine on.", detail(result, "application"));
        assertEquals("The app may make changes.", detail(result, "settings"));
        assertEquals("Waiting for the app on port 19791, with 1 connected now.", detail(result, "remote connection"));
        assertEquals("2 actions.", detail(result, "actions"));
        assertEquals("Bitwig shows its ARRANGE layout, with the arranger timeline shown and the Arrange layout's clip launcher shown; "
                     + "no clip is selected in the arranger, and the play start is at beat 0.",
                     detail(result, "arranger"));
        assertEquals("No clip is selected in the arranger; no clip is selected in the clip launcher.", detail(result, "clip notes"));

        assertJson(obj("why", "asked", "ok", true, "extension", MusicOrganiserExtensionDefinition.VERSION, "link", 3, "product", "Bitwig Studio", "bitwig", "6.1.2", "api", 25,
                       "apiUsed", 23, "java", System.getProperty("java.version"), "platform", "WINDOWS", "changes", true, "port", 19791,
                       "connections", 1, "folder", reports.toString()),
                   pick(result, "why", "ok", "extension", "link", "product", "bitwig", "api", "apiUsed", "java", "platform", "changes", "port",
                        "connections", "folder"));
        assertEquals("2026-10-14T09:52:42.000Z", result.get("time"));

        // The file has the same, without its own name.
        Path file = Paths.get((String) result.get("file"));
        assertEquals(reports.resolve("2026-10-14T095242.000Z-selfcheck.json"), file);
        Map<String, Object> written = map(Json.parse(Files.readString(file, StandardCharsets.US_ASCII)));
        result.remove("file");
        assertEquals(result, written);
    }

    @Test
    @DisplayName("checks itself a second after Bitwig starts it, once Bitwig has sent what it watches")
    void checksItselfAtStart() throws IOException {
        bitwig = FakeBitwig.start(LinkTest.song(), reports);
        bitwig.clock.advance(999);
        assertEquals(List.of(), selfChecks());
        bitwig.clock.advance(1);
        List<Path> files = selfChecks();
        assertEquals(List.of(reports.resolve("2026-10-14T095243.000Z-selfcheck.json")), files);
        Map<String, Object> check = map(Json.parse(Files.readString(files.get(0))));
        assertJson(obj("why", "start", "ok", true, "changes", false, "connections", 0), pick(check, "why", "ok", "changes", "connections"));
        assertEquals(CHECKS, names(check));
        assertEquals(List.of(), bitwig.errors);
    }

    @Test
    @DisplayName("keeps working when part of Bitwig's API fails, and says which part and why")
    void keepsWorkingWhenAPartFails() {
        Model m = LinkTest.song();
        m.broken.add("createMasterRecorder");
        m.broken.add("createLastClickedParameter");
        start(m);
        bitwig.allowChanges(true);

        assertEquals(true, app.ask(obj("id", 1, "cmd", "project")).get("ok"));
        assertEquals("This Bitwig didn't let the link follow the last clicked parameter. A selftest says why.",
                     error(app.ask(obj("id", 2, "cmd", "clicked"))));
        assertEquals("This Bitwig didn't let the link use its master recorder. A selftest says why.",
                     error(app.ask(obj("id", 3, "cmd", "recorder", "do", "start"))));

        Map<String, Object> result = map(app.ask(obj("id", 4, "cmd", "selftest")).get("result"));
        assertEquals(false, result.get("ok"));
        assertEquals(CHECKS, names(result));
        for (Object check : listAt(result, "checks")) {
            boolean broken = List.of("last clicked parameter", "master recorder").contains(at(check, "name"));
            assertEquals(!broken, at(check, "ok"), Json.write(check));
            if (broken) assertMatches("^Couldn't set it up: java.lang.AbstractMethodError: ", at(check, "detail"));
        }
        assertMatches("couldn't set up the last clicked parameter", bitwig.errors.get(0));
        assertMatches("couldn't set up the master recorder", bitwig.errors.get(1));
    }

    @Test
    @DisplayName("still says hello without its track bank, and answers what needs it with an error")
    void saysHelloWithoutItsTrackBank() {
        Model m = LinkTest.song();
        m.broken.add("createTrackBank");
        start(m);
        assertEquals("Night Drive", at(app.ask(obj("id", 1, "cmd", "hello")), "result", "project"));
        assertEquals(true, app.ask(obj("id", 2, "cmd", "actions")).get("ok"));
        assertMatches("^The Bitwig extension failed: ", error(app.ask(obj("id", 3, "cmd", "project"))));

        Map<String, Object> result = map(app.ask(obj("id", 4, "cmd", "selftest")).get("result"));
        List<Object> failed = listAt(result, "checks").stream().filter(c -> !(Boolean) at(c, "ok")).map(c -> at(c, "name"))
                                                      .collect(Collectors.toList());
        // Recording into the arranger (extension 3.9) needs the tracks too.
        assertEquals(List.of("track bank", "Sampler and Note Transpose pitch", "track IDs", "recording into the arranger"), failed);
        assertEquals("Couldn't set it up: org.musicorganiser.bitwig.LinkError: The track bank wasn't made, so there's no track to record.",
                     detail(result, "recording into the arranger"));
    }

    @Test
    @DisplayName("says so when Bitwig gave it another port, or offers an older API")
    void saysWhatsWrongWithTheConnectionAndApi() {
        Model m = LinkTest.song();
        m.portInUse = 50123;
        m.api = 22;
        bitwig = FakeBitwig.start(m, reports);
        Map<String, Object> result = bitwig.link.selfCheck("test");
        assertEquals(false, result.get("ok"));
        assertEquals("Bitwig offers API 22; the link needs 23.", detail(result, "API version"));
        assertEquals("Port 19791 is taken, so Bitwig gave the link port 50123, where the app doesn't look. " + Link.TWICE, detail(result, "remote connection"));
        assertEquals(List.of("API version", "remote connection"),
                     listAt(result, "checks").stream().filter(c -> !(Boolean) at(c, "ok")).map(c -> at(c, "name")).collect(Collectors.toList()));
    }

    @Test
    @DisplayName("asks Bitwig again which port it gave the link, when it didn't say at first")
    void asksAgainWhichPort() {
        Model m = LinkTest.song();
        m.portUnknown = 2;
        bitwig = FakeBitwig.start(m, reports);
        // Bitwig still doesn't know: that's not the port being taken.
        assertEquals("Waiting for the app on port 19791, though Bitwig hasn't said yet which port it gave the link.",
                     detail(bitwig.link.selfCheck("test"), "remote connection"));
        Map<String, Object> result = bitwig.link.selfCheck("test");
        assertEquals(true, result.get("ok"));
        assertEquals("Waiting for the app on port 19791, with 0 connected now.", detail(result, "remote connection"));
        assertEquals(19791, ((Number) result.get("port")).intValue());
        assertEquals(List.of(), bitwig.popups);
    }

    @Test
    @DisplayName("keeps working when it can't write its reports, and the self-check says why")
    void keepsWorkingWithoutItsReports() throws IOException {
        Path inTheWay = Files.writeString(reports.resolve("in the way"), "x");
        bitwig = FakeBitwig.start(LinkTest.song(), inTheWay.resolve("bitwig"));
        app = bitwig.connect();
        assertEquals("hello", app.take().get(0).get("event"));
        assertEquals(true, app.ask(obj("id", 1, "cmd", "project")).get("ok"));

        Map<String, Object> result = map(app.ask(obj("id", 2, "cmd", "selftest")).get("result"));
        assertEquals(false, result.get("ok"));
        assertNull(result.get("file"));
        assertMatches("^Can't write in ", detail(result, "reports folder"));
        assertEquals(List.of("in the way"), list(reports));
    }

    // ---- Helpers ----

    @SuppressWarnings("unchecked")
    static Map<String, Object> map(Object value) {
        return (Map<String, Object>) value;
    }

    static List<Object> names(Map<String, Object> result) {
        return listAt(result, "checks").stream().map(c -> at(c, "name")).collect(Collectors.toList());
    }

    static Object detail(Map<String, Object> result, String name) {
        for (Object check : listAt(result, "checks"))
            if (name.equals(at(check, "name"))) return at(check, "detail");
        throw new AssertionError("No check called " + name);
    }

    static Map<String, Object> pick(Map<String, Object> map, String... keys) {
        Map<String, Object> picked = new LinkedHashMap<>();
        for (String key : keys) picked.put(key, map.get(key));
        return picked;
    }

    private List<Path> selfChecks() throws IOException {
        try (Stream<Path> files = Files.list(reports)) {
            return files.filter(f -> f.getFileName().toString().endsWith(Reports.SELF_CHECK)).sorted().collect(Collectors.toList());
        }
    }

    static List<String> list(Path folder) throws IOException {
        try (Stream<Path> files = Files.list(folder)) {
            return files.map(f -> f.getFileName().toString()).sorted().collect(Collectors.toList());
        }
    }
}
