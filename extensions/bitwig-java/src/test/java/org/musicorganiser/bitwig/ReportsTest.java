package org.musicorganiser.bitwig;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.time.Clock;
import java.time.Instant;
import java.time.LocalDate;
import java.time.ZoneOffset;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.stream.Collectors;
import java.util.stream.Stream;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

/** Where the reports go and what's in them, without Bitwig (the link's own use of them is tested with a stand-in Bitwig). */
class ReportsTest {
    static final Clock CLOCK = Clock.fixed(Instant.parse("2026-10-14T09:52:42.123Z"), ZoneOffset.UTC);

    @TempDir
    Path temp;

    @Test
    void goesInTheAppsDataFolderAsMoProbesReportsDo() {
        assertEquals(Paths.get("C:/Users/Solomon/AppData/Local", "MusicOrganiser", "reports", "bitwig"),
                     Reports.defaultFolder("Windows 11", Map.of("LOCALAPPDATA", "C:/Users/Solomon/AppData/Local"), "C:/Users/Solomon", "/tmp"));
        assertEquals(Paths.get("/Users/sol/Library/Application Support/MusicOrganiser/reports/bitwig"),
                     Reports.defaultFolder("Mac OS X", Map.of(), "/Users/sol", "/tmp"));
        assertEquals(Paths.get("/data/MusicOrganiser/reports/bitwig"),
                     Reports.defaultFolder("Linux", Map.of("XDG_DATA_HOME", "/data"), "/home/sol", "/tmp"));
        assertEquals(Paths.get("/home/sol/.local/share/MusicOrganiser/reports/bitwig"),
                     Reports.defaultFolder("Linux", Map.of("XDG_DATA_HOME", ""), "/home/sol", "/tmp"));
        assertEquals(Paths.get("/tmp/MusicOrganiser/reports/bitwig"), Reports.defaultFolder("Windows 10", Map.of(), "", "/tmp"),
                     "the temporary folder when there's nothing better");
    }

    @Test
    void logsOneLineForEachEntry() throws IOException {
        Reports reports = new Reports(temp.resolve("reports"), CLOCK);
        Map<String, Object> line = new LinkedHashMap<>();
        line.put("cmd", "project");
        line.put("ms", 3L);
        line.put("ok", true);
        reports.log(line);
        reports.log(Map.of("event", "connected"));
        List<String> lines = Files.readAllLines(temp.resolve("reports").resolve("2026-10-14-link.jsonl"), StandardCharsets.US_ASCII);
        assertEquals(List.of("{\"time\":\"2026-10-14T09:52:42.123Z\",\"cmd\":\"project\",\"ms\":3,\"ok\":true}",
                             "{\"time\":\"2026-10-14T09:52:42.123Z\",\"event\":\"connected\"}"), lines);
        assertNull(reports.problem());
    }

    @Test
    void writesASelfCheckNamedByTheTime() throws IOException {
        Reports reports = new Reports(temp, CLOCK);
        Path file = reports.writeSelfCheck(Map.of("ok", true));
        assertEquals(temp.resolve("2026-10-14T095242.123Z-selfcheck.json"), file);
        assertEquals("{\n  \"ok\": true\n}\n", Files.readString(file));
    }

    @Test
    void saysWhenItCantWriteWithoutBreakingAnything() throws IOException {
        Path inTheWay = Files.writeString(temp.resolve("not a folder"), "x");
        Reports reports = new Reports(inTheWay.resolve("reports"), CLOCK);
        reports.log(Map.of("cmd", "hello"));
        assertNotNull(reports.problem());
        assertNull(reports.writeSelfCheck(Map.of()));
        assertTrue(reports.checkWritable().startsWith("Can't write in "));
        assertNull(new Reports(temp, CLOCK).checkWritable());
        assertEquals(List.of("not a folder"), names(temp), "and leaves no test file behind");
    }

    @Test
    void deletesItsOwnReportsAfterTwoWeeks() throws IOException {
        LocalDate today = LocalDate.of(2026, 10, 14);
        for (String name : new String[] {
            "2026-09-29-link.jsonl", "2026-09-30-link.jsonl", "2026-10-14-link.jsonl",
            "2026-09-01T120000.000Z-selfcheck.json", "2026-10-13T120000.000Z-selfcheck.json",
            "2026-01-01 notes.txt", "probe.jsonl", "link.jsonl", "not-a-date-link.jsonl"
        }) Files.writeString(temp.resolve(name), "x");
        assertEquals(LocalDate.of(2026, 9, 30), today.minusDays(Reports.KEEP_DAYS));
        assertEquals(2, new Reports(temp, CLOCK).tidy());
        assertEquals(List.of("2026-01-01 notes.txt", "2026-09-30-link.jsonl", "2026-10-13T120000.000Z-selfcheck.json",
                             "2026-10-14-link.jsonl", "link.jsonl", "not-a-date-link.jsonl", "probe.jsonl"), names(temp));
        assertEquals(0, new Reports(temp.resolve("missing"), CLOCK).tidy());
    }

    private static List<String> names(Path folder) throws IOException {
        try (Stream<Path> files = Files.list(folder)) {
            return files.map(f -> f.getFileName().toString()).sorted().collect(Collectors.toList());
        }
    }
}
