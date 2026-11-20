package org.musicorganiser.bitwig;

import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.DirectoryStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.nio.file.StandardOpenOption;
import java.time.Clock;
import java.time.LocalDate;
import java.time.ZoneOffset;
import java.time.format.DateTimeFormatter;
import java.time.format.DateTimeParseException;
import java.util.LinkedHashMap;
import java.util.Locale;
import java.util.Map;
import java.util.function.Supplier;

/**
 * The extension's reports, in the app's data folder next to MO Probe's, so what happened in a real Bitwig can
 * be read afterwards. The data folder starts out here:
 *
 * <pre>
 *   Windows  %LOCALAPPDATA%\MusicOrganiser\reports\bitwig
 *   macOS    ~/Library/Application Support/MusicOrganiser/reports/bitwig
 *   Linux    $XDG_DATA_HOME/MusicOrganiser/reports/bitwig (or ~/.local/share/...)
 * </pre>
 *
 * When the app's Settings moved the data folder to another drive, the reports go there too (extension 3.26):
 * {@code MusicOrganiser-folder.txt} beside the first folder names the new one, as the app reads it
 * (crates/mo-app/src/paths.rs). The folder is looked up when Bitwig starts the extension, and again when the app or a
 * plugin connects, at most once a minute ({@link #refresh}; the first minute after the start counts as looked).
 *
 * {@code <date>-link.jsonl} has a line for each command the app sent (time, cmd, ms, ok, error) and for
 * connections coming and going; {@code <time>-selfcheck.json} is a self-check of the API features the link
 * uses. Nothing in them comes from a project beyond names and the paths the app sent.
 *
 * Writing is best effort: a file that can't be written never breaks the link. The last problem is kept for the
 * self-check to report.
 */
final class Reports {
    /** How long the extension keeps its own reports. */
    static final int KEEP_DAYS = 14;
    static final String LOG = "-link.jsonl";
    static final String SELF_CHECK = "-selfcheck.json";
    /** The app's data folder's name, where the system keeps an app's data. */
    static final String DATA_FOLDER = "MusicOrganiser";
    /** Beside the first data folder: one line, the folder the app's data moved to (the app's paths::POINTER_FILE). */
    static final String POINTER_FILE = "MusicOrganiser-folder.txt";
    /** In a folder a move finished in (the app's paths::MOVED_IN_FILE): a pointer to a folder without it isn't followed. */
    static final String MOVED_IN_FILE = "moved-in.json";
    /**
     * In a folder the installer started the data in on a new PC (the app's install_choice::STARTED_HERE_FILE,
     * crates/mo-app/src/install_choice.rs): a pointer to a folder with it is followed too, as the app does
     * (extension 3.33, the 19 Nov review's LK-2).
     */
    static final String STARTED_HERE_FILE = "started-here.txt";
    /** A pointer file is one path: anything bigger isn't one, and isn't read. */
    private static final long POINTER_MOST_BYTES = 64 * 1024;

    /** A self-check's file name starts with the time, without colons, which Windows doesn't allow in names. */
    private static final DateTimeFormatter STAMP = DateTimeFormatter.ofPattern("yyyy-MM-dd'T'HHmmss.SSS'Z'", Locale.ROOT)
                                                                    .withZone(ZoneOffset.UTC);
    private static final DateTimeFormatter TIME = DateTimeFormatter.ofPattern("yyyy-MM-dd'T'HH:mm:ss.SSS'Z'", Locale.ROOT)
                                                                   .withZone(ZoneOffset.UTC);

    private Path folder;
    /** Finds the folder again ({@link #refresh}), or null for one that stays where it is. */
    private final Supplier<Path> find;
    /** How long a look for the folder holds before {@link #refresh} looks again. */
    static final long LOOK_AGAIN_MS = 60_000;
    /** When the folder was last looked for, by the clock; none yet at first. */
    private long lookedAt = Long.MIN_VALUE;
    private final Clock clock;
    private String problem;

    Reports(Path folder, Clock clock) {
        this.folder = folder;
        this.find = null;
        this.clock = clock;
    }

    /** Reports in the folder {@code find} gives, asked again at each {@link #refresh}. */
    Reports(Supplier<Path> find, Clock clock) {
        this.folder = find.get();
        this.find = find;
        this.clock = clock;
        this.lookedAt = clock.millis();
    }

    /**
     * Looks again where the reports go (extension 3.30): the app's Settings can move its data folder while Bitwig
     * stays open, and the reports then follow it instead of going on in the old folder until Bitwig restarts (and
     * making it again once you've removed it). Says whether the folder changed.
     */
    boolean refresh() {
        if (find == null) return false;
        // Not at every connection: looking reads two small files, one of them on the drive the folder moved to, which
        // may be slow to wake, and this runs on Bitwig's own thread.
        long now0 = clock.millis();
        if (lookedAt != Long.MIN_VALUE && now0 - lookedAt < LOOK_AGAIN_MS) return false;
        lookedAt = now0;
        try {
            Path now = find.get();
            if (now == null || now.equals(folder)) return false;
            folder = now;
            return true;
        } catch (RuntimeException e) {
            return false;
        }
    }

    Path folder() {
        return folder;
    }

    /** The last thing that went wrong writing a report, or null if nothing has. */
    String problem() {
        return problem;
    }

    /** Where reports go on this computer. */
    static Path defaultFolder() {
        return defaultFolder(System.getProperty("os.name", ""), System.getenv(), System.getProperty("user.home", ""),
                             System.getProperty("java.io.tmpdir", "."));
    }

    /** Where reports go, worked out as MO Probe does (native/mo-probe/src/log.cpp) from the system and its settings. */
    static Path defaultFolder(String os, Map<String, String> env, String home, String temp) {
        String system = os.toLowerCase(Locale.ROOT);
        Path base = null;
        if (system.startsWith("windows")) {
            base = path(env.get("LOCALAPPDATA"));
        } else if (system.startsWith("mac") || system.contains("darwin")) {
            Path h = path(home);
            if (h != null) base = h.resolve("Library").resolve("Application Support");
        } else {
            base = path(env.get("XDG_DATA_HOME"));
            Path h = path(home);
            if (base == null && h != null) base = h.resolve(".local").resolve("share");
        }
        if (base == null) base = Paths.get(temp);
        return dataFolder(base.resolve(DATA_FOLDER)).resolve("reports").resolve("bitwig");
    }

    /**
     * The app's data folder, given the one it starts out with: the folder the app's Settings moved it to, or the
     * installer started it in, when the pointer file beside {@code first} names one and the move there finished (the
     * app leaves {@link #MOVED_IN_FILE} in it) or the installer began there ({@link #STARTED_HERE_FILE}); else
     * {@code first}. The installer writes the pointer with a byte-order mark, which is skipped. The same rule as the app's own (crates/mo-app/src/paths.rs), so the reports never
     * go to a folder the app isn't using: with the new drive missing, both use the first folder.
     */
    static Path dataFolder(Path first) {
        try {
            Path pointer = first.resolveSibling(POINTER_FILE);
            if (!Files.isRegularFile(pointer) || Files.size(pointer) > POINTER_MOST_BYTES) return first;
            String text = new String(Files.readAllBytes(pointer), StandardCharsets.UTF_8);
            // String.trim() keeps a byte-order mark, and a path can't start with one.
            if (text.startsWith("\uFEFF")) text = text.substring(1);
            text = text.trim();
            if (text.isEmpty()) return first;
            Path named = Paths.get(text);
            boolean theirs = Files.isRegularFile(named.resolve(MOVED_IN_FILE)) || Files.isRegularFile(named.resolve(STARTED_HERE_FILE));
            return named.isAbsolute() && theirs ? named : first;
        } catch (IOException | RuntimeException e) {
            // A pointer that can't be read or names no path: the app falls back to the first folder too.
            return first;
        }
    }

    private static Path path(String text) {
        return text == null || text.isEmpty() ? null : Paths.get(text);
    }

    /** The time now, as the reports write it (UTC, to the millisecond). */
    String now() {
        return TIME.format(clock.instant());
    }

    /** Adds a line to today's log (UTC), with the time in front. */
    void log(Map<String, Object> entry) {
        Map<String, Object> line = new LinkedHashMap<>();
        line.put("time", now());
        line.putAll(entry);
        String file = LocalDate.ofInstant(clock.instant(), ZoneOffset.UTC) + LOG;
        try {
            Files.createDirectories(folder);
            Files.write(folder.resolve(file), (Json.write(line) + "\n").getBytes(StandardCharsets.US_ASCII),
                        StandardOpenOption.CREATE, StandardOpenOption.APPEND, StandardOpenOption.WRITE);
        } catch (IOException | RuntimeException e) {
            problem = "Couldn't add to " + folder.resolve(file) + ": " + e;
        }
    }

    /** Writes a self-check report, and returns the file, or null if it couldn't. */
    Path writeSelfCheck(Map<String, Object> report) {
        Path file = folder.resolve(STAMP.format(clock.instant()) + SELF_CHECK);
        try {
            Files.createDirectories(folder);
            Files.write(file, (Json.pretty(report) + "\n").getBytes(StandardCharsets.US_ASCII));
            return file;
        } catch (IOException | RuntimeException e) {
            problem = "Couldn't write " + file + ": " + e;
            return null;
        }
    }

    /** Whether the folder takes a new file, or why not. Null means it does. */
    String checkWritable() {
        try {
            Files.createDirectories(folder);
            Path test = Files.createTempFile(folder, ".write-test-", ".tmp");
            Files.delete(test);
            return null;
        } catch (IOException | RuntimeException e) {
            return "Can't write in " + folder + ": " + e;
        }
    }

    /**
     * Deletes the extension's own reports from before the last {@link #KEEP_DAYS} days (by the date their names
     * start with), leaving anything else in the folder alone. Returns how many went.
     */
    int tidy() {
        if (!Files.isDirectory(folder)) return 0;
        LocalDate oldest = LocalDate.ofInstant(clock.instant(), ZoneOffset.UTC).minusDays(KEEP_DAYS);
        int deleted = 0;
        try (DirectoryStream<Path> files = Files.newDirectoryStream(folder)) {
            for (Path file : files) {
                LocalDate date = dateOf(file.getFileName().toString());
                if (date == null || !date.isBefore(oldest)) continue;
                try {
                    Files.delete(file);
                    deleted++;
                } catch (IOException e) {
                    problem = "Couldn't delete " + file + ": " + e;
                }
            }
        } catch (IOException | RuntimeException e) {
            problem = "Couldn't tidy " + folder + ": " + e;
        }
        return deleted;
    }

    /** The date a report of the extension's own is from, or null for any other file. */
    static LocalDate dateOf(String name) {
        if (!name.endsWith(LOG) && !name.endsWith(SELF_CHECK)) return null;
        if (name.length() < 10) return null;
        try {
            return LocalDate.parse(name.substring(0, 10));
        } catch (DateTimeParseException e) {
            return null;
        }
    }
}
