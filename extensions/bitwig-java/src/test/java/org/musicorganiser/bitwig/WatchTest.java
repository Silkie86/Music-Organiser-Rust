package org.musicorganiser.bitwig;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.musicorganiser.bitwig.FakeBitwig.playing;
import static org.musicorganiser.bitwig.Messages.assertJson;
import static org.musicorganiser.bitwig.Messages.at;
import static org.musicorganiser.bitwig.Messages.error;
import static org.musicorganiser.bitwig.Messages.list;
import static org.musicorganiser.bitwig.Messages.obj;

import com.bitwig.extension.controller.api.PlayingNote;
import java.nio.file.Path;
import java.util.List;
import java.util.Map;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;
import org.musicorganiser.bitwig.FakeBitwig.App;
import org.musicorganiser.bitwig.FakeBitwig.Marker;
import org.musicorganiser.bitwig.FakeBitwig.Model;

/** The watch feed (extension 3.24): what changes in Bitwig, sent to the app as it changes. */
class WatchTest {
    @TempDir
    Path reports;

    FakeBitwig bitwig;
    App app;

    void start(Model model) {
        bitwig = FakeBitwig.start(model, reports);
        app = bitwig.connect();
        app.take();
    }

    /** Starts watching, and returns the first event: the whole picture. */
    Map<String, Object> watch() {
        List<Map<String, Object>> fresh = sent(obj("id", 1, "cmd", "watch"));
        assertEquals(2, fresh.size(), "the answer, then the whole picture");
        assertEquals(true, at(fresh.get(0), "result", "watching"));
        assertEquals("watch", fresh.get(1).get("event"));
        return fresh.get(1);
    }

    List<Map<String, Object>> sent(Object request) {
        app.send(request);
        return app.take();
    }

    /** What the feed sent while `ms` passed. */
    List<Map<String, Object>> after(long ms) {
        bitwig.clock.advance(ms);
        return app.take();
    }

    @Test
    @DisplayName("answers with what it gives, then sends the whole picture straight away")
    void sendsTheWholePictureFirst() {
        Model m = LinkTest.song();
        m.tracks.get(1).id("bass-id");
        m.markers.add(new Marker("Chorus", 32, 1, 0, 0));
        m.markers.add(new Marker("Intro", 0, 0, 0, 1));
        start(m);
        app.send(obj("id", 1, "cmd", "watch"));
        List<Map<String, Object>> fresh = app.take();
        assertJson(obj("id", 1, "ok", true,
                       "result", obj("watching", true, "every", 50,
                                     "sections", list("project", "tracks", "selected", "transport", "markers", "notes", "levels", "groove"))),
                   fresh.get(0));
        Map<String, Object> first = fresh.get(1);
        assertEquals("watch", first.get("event"));
        assertTrue(first.get("time") instanceof Number);
        Object result = first.get("result");
        assertEquals(List.of("project", "transport", "tracks", "selected", "markers", "levels", "groove", "notes"),
                     List.copyOf(((Map<?, ?>) result).keySet()));
        assertJson(obj("name", "Night Drive", "modified", false), at(result, "project"));
        assertJson(obj("playing", false, "position", 0, "playStart", 0, "tempo", 124, "timeSignature", "7/8"), at(result, "transport"));
        assertEquals(3, ((List<?>) at(result, "tracks")).size());
        assertEquals("bass-id", at(result, "tracks", 1, "id"));
        assertEquals("Bass", at(result, "tracks", 1, "name"));
        assertEquals("#ff8000", at(result, "tracks", 1, "color"));
        assertEquals("Serum", at(result, "tracks", 1, "devices", 0, "name"));
        assertFalse(((Map<?, ?>) at(result, "tracks", 1)).containsKey("volume"), "automation moves the volume all the time");
        assertEquals(1.0, ((Number) at(result, "selected", "track")).doubleValue());
        assertEquals("Bass", at(result, "selected", "name"));
        // By where they are, not as Bitwig lists them.
        assertJson(list(obj("name", "Intro", "at", 0, "color", "#0000ff"), obj("name", "Chorus", "at", 32, "color", "#ff0000")),
                   at(result, "markers"));
        assertJson(list(), at(result, "notes"));
        assertJson(list(), at(result, "levels"));
        assertJson(obj("on", false, "shuffle", 0.5, "rate", "1/16"), at(result, "groove"));
        assertEquals(List.of(), bitwig.errors);
    }

    @Test
    @DisplayName("sends nothing while nothing changes")
    void staysQuietWhileNothingChanges() {
        start(LinkTest.song());
        watch();
        assertEquals(List.of(), after(Watch.KEEP_ALIVE - Watch.TICK));
    }

    @Test
    @DisplayName("sends an empty event when nothing has changed for a while, so the app knows it still answers")
    void keepsTheAppPosted() {
        start(LinkTest.song());
        watch();
        assertEquals(List.of(), after(Watch.KEEP_ALIVE - Watch.TICK));
        List<Map<String, Object>> fresh = after(Watch.TICK);
        assertEquals(1, fresh.size());
        assertJson(obj(), fresh.get(0).get("result"));
        // Counted from the last thing sent: a change puts the next one off.
        bitwig.meter(1, 500, 500);
        assertEquals(1, after(2 * Watch.TICK).size());
        assertEquals(List.of(), after(Watch.KEEP_ALIVE - 2 * Watch.TICK));
    }

    @Test
    @DisplayName("sends only what changed: the notes a track plays, on the next look")
    void sendsTheNotesPlaying() {
        start(LinkTest.song());
        watch();
        bitwig.trackItems.get(1).notes.live = () -> new PlayingNote[] {playing(64, 90), playing(60, 100)};
        List<Map<String, Object>> fresh = after(Watch.TICK);
        assertEquals(1, fresh.size());
        assertJson(obj("notes", list(obj("track", 1, "notes", list(list(60, 100), list(64, 90))))), fresh.get(0).get("result"));
        // The same notes still playing aren't sent again; when they stop, the empty list is.
        assertEquals(List.of(), after(500));
        bitwig.trackItems.get(1).notes.live = () -> new PlayingNote[0];
        assertJson(obj("notes", list()), after(Watch.TICK).get(0).get("result"));
    }

    @Test
    @DisplayName("sends each track's level, the louder of its meters, at most every 100 ms")
    void sendsLevels() {
        start(LinkTest.song());
        watch();
        bitwig.meter(1, 500, 250);
        assertEquals(List.of(), after(Watch.TICK), "levels go with the slow looks");
        assertJson(obj("levels", list(list(1, 0.5))), after(Watch.TICK).get(0).get("result"));
        bitwig.meter(1, 0, 0);
        assertJson(obj("levels", list()), after(2 * Watch.TICK).get(0).get("result"));
    }

    @Test
    @DisplayName("sends the cue markers when one is added, renamed or moved")
    void sendsMarkers() {
        start(LinkTest.song());
        watch();
        bitwig.model.markers.add(new Marker("Verse", 16, 0, 1, 0));
        assertJson(obj("markers", list(obj("name", "Verse", "at", 16, "color", "#00ff00"))),
                   after(2 * Watch.TICK).get(0).get("result"));
        bitwig.model.markers.set(0, new Marker("Verse 1", 16.5, 0, 1, 0));
        assertJson(obj("markers", list(obj("name", "Verse 1", "at", 16.5, "color", "#00ff00"))),
                   after(2 * Watch.TICK).get(0).get("result"));
    }

    @Test
    @DisplayName("sends the song's Groove when it's turned on, its shuffle moves or its rate changes (extension 3.27)")
    void sendsTheGroove() {
        start(LinkTest.song());
        watch();
        bitwig.model.grooveOn = true;
        bitwig.model.grooveShuffle = 0.6204;
        assertJson(obj("groove", obj("on", true, "shuffle", 0.62, "rate", "1/16")), after(2 * Watch.TICK).get(0).get("result"));
        bitwig.model.grooveShuffle = 0.62041;
        assertEquals(List.of(), after(2 * Watch.TICK), "a knob that barely moved sends nothing");
        bitwig.model.grooveRate = "1/8";
        assertJson(obj("groove", obj("on", true, "shuffle", 0.62, "rate", "1/8")), after(2 * Watch.TICK).get(0).get("result"));
    }

    @Test
    @DisplayName("gives the Groove's rate as Bitwig shows it, and never guesses one")
    void readsTheGrooveRate() {
        assertEquals("1/16", Watch.grooveRate(" 1/16 "));
        assertEquals("1/8", Watch.grooveRate("1/8"));
        assertEquals("", Watch.grooveRate(""));
        assertEquals("1/32", Watch.grooveRate("1/32"));
        assertEquals(0.0, Watch.amount(Double.NaN));
        assertEquals(1.0, Watch.amount(1.2));
    }

    @Test
    @DisplayName("gives the rest when this Bitwig has no Groove")
    void goesOnWithoutTheGroove() {
        Model m = LinkTest.song();
        m.broken.add("createGroove");
        start(m);
        Object result = watch().get("result");
        assertFalse(((Map<?, ?>) result).containsKey("groove"));
        assertTrue(((Map<?, ?>) result).containsKey("levels"));
        assertEquals(false, at(app.ask(obj("id", 2, "cmd", "hello")), "result", "watch").toString().contains("groove"));
    }

    @Test
    @DisplayName("stops with on: false, and with the connection")
    void stops() {
        start(LinkTest.song());
        watch();
        assertJson(obj("id", 2, "ok", true, "result", obj("watching", false)), app.ask(obj("id", 2, "cmd", "watch", "on", false)));
        bitwig.meter(1, 500, 500);
        assertEquals(List.of(), after(1000));

        // Another app watching, then gone: nothing breaks, and nothing goes to the one that stopped.
        App other = bitwig.connect();
        other.take();
        other.send(obj("id", 1, "cmd", "watch"));
        assertEquals(2, other.take().size());
        other.close();
        bitwig.meter(1, 100, 100);
        assertEquals(List.of(), after(1000));
        assertEquals(List.of(), bitwig.errors);
    }

    @Test
    @DisplayName("tells each app what changed for it, from the whole picture when it starts")
    void eachAppHasItsOwn() {
        start(LinkTest.song());
        watch();
        bitwig.meter(1, 500, 500);
        after(2 * Watch.TICK);
        App other = bitwig.connect();
        other.take();
        other.send(obj("id", 1, "cmd", "watch"));
        List<Map<String, Object>> fresh = other.take();
        assertJson(list(list(1, 0.5)), at(fresh.get(1), "result", "levels"));
        assertEquals(List.of(), after(1000));
        assertEquals(List.of(), other.take());
    }

    @Test
    @DisplayName("refuses an on that isn't true or false")
    void refusesAnOddOn() {
        start(LinkTest.song());
        assertEquals("on has to be true or false.", error(app.ask(obj("id", 1, "cmd", "watch", "on", "yes"))));
    }

    @Test
    @DisplayName("gives the rest when this Bitwig has no level meters, and its selftest says why")
    void goesOnWithoutLevels() {
        Model m = LinkTest.song();
        m.broken.add("addVuMeterObserver");
        start(m);
        Object result = watch().get("result");
        assertFalse(((Map<?, ?>) result).containsKey("levels"));
        assertTrue(((Map<?, ?>) result).containsKey("notes"));
        assertEquals(false, at(app.ask(obj("id", 2, "cmd", "hello")), "result", "watch").toString().contains("levels"));
    }
}
