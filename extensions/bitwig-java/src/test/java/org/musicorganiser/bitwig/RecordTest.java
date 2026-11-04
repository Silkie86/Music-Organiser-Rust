package org.musicorganiser.bitwig;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.musicorganiser.bitwig.FakeBitwig.action;
import static org.musicorganiser.bitwig.FakeBitwig.track;
import static org.musicorganiser.bitwig.Messages.assertJson;
import static org.musicorganiser.bitwig.Messages.assertMatches;
import static org.musicorganiser.bitwig.Messages.at;
import static org.musicorganiser.bitwig.Messages.error;
import static org.musicorganiser.bitwig.Messages.list;
import static org.musicorganiser.bitwig.Messages.listAt;
import static org.musicorganiser.bitwig.Messages.obj;

import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.Map;
import java.util.function.Consumer;
import java.util.stream.Collectors;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;
import org.musicorganiser.bitwig.FakeBitwig.App;
import org.musicorganiser.bitwig.FakeBitwig.Model;
import org.musicorganiser.bitwig.FakeBitwig.Recorded;
import org.musicorganiser.bitwig.FakeBitwig.TrackSpec;

/**
 * Recording a launcher clip into the arranger (extension 3.9): the link sets Bitwig up so nothing but the clip is recorded
 * and nothing is heard, turns arranger record on, launches the clip, stops its track on the clip's last bar, turns record
 * off, plays the arranger back to hear what went in, and puts everything back. The stand-in Bitwig plays (its play position
 * runs on its clock), records what a launcher clip plays into the arranger while record is on, and plays that back; switches
 * in its model make it ignore a setting or stopping a track, record nothing, hide a track's notes, and so on.
 */
class RecordTest {
    /** Bar 5 in 4/4, where the clip is recorded: four bars of four beats from the start. */
    static final double BAR_5 = 16;
    /** The clip: two bars of six notes, each a different key. */
    static final String CLIP = "C:\\Tests\\Rec Clip.mid";
    static final double LENGTH = 8;
    static final List<double[]> NOTES = List.of(new double[] {0, 1, 57}, new double[] {1.5, 1, 60}, new double[] {3, 1, 64},
                                                new double[] {4, 1, 62}, new double[] {5.5, 1, 65}, new double[] {7, 0.5, 69});
    static final List<Integer> KEYS = List.of(57, 60, 64, 62, 65, 69);

    @TempDir
    Path reports;

    FakeBitwig bitwig;
    App app;
    Model model;

    /**
     * The live tests' project: an armed track, the track with the clip to record (ID "rec"), and the master; changes allowed,
     * and IDs for new tracks.
     */
    void studio(Consumer<Model> extra) {
        Model m = new Model();
        m.project = "MO Live Test";
        m.selectedTrack = "Bass";
        m.tracks(
            track("Bass", "Instrument").id("bass").slots("C:\\MIDI\\Bass.mid"),
            track("Keys", "Instrument").id("keys").armed(),
            track("Rec", "Instrument").id("rec").slots(CLIP),
            track("Master", "Master").id("master"));
        m.clipNotes.put(CLIP, NOTES);
        m.clipLength.put(CLIP, LENGTH);
        m.newTrackIds = true;
        m.actions.add(action(Link.TOGGLE_ARRANGER, "Toggle Arranger Timeline Panel", "Panel Management"));
        m.actions.add(action(Link.FOCUS_ARRANGER, "Focus/toggle Arranger Timeline Panel", "Panel Management"));
        m.actions.add(action(Link.FOCUS_LAUNCHER, "Focus/toggle Clip Launcher Panel", "Panel Management"));
        extra.accept(m);
        model = m;
        bitwig = FakeBitwig.start(m, reports);
        bitwig.allowChanges(true);
        app = bitwig.connect();
        app.take();
    }

    void studio() {
        studio(m -> {});
    }

    static Map<String, Object> record(Object id, Object trackId, Object at, Object... more) {
        Map<String, Object> request = obj("id", id, "cmd", "record", "onlyIn", "MO Live Test", "trackId", trackId, "slot", 0, "at", at, "length", LENGTH);
        for (int i = 0; i < more.length; i += 2) request.put((String) more[i], more[i + 1]);
        return request;
    }

    Map<String, Object> result(Map<String, Object> request) {
        Map<String, Object> reply = app.waitFor(request, 60000);
        assertEquals(true, reply.get("ok"), () -> Json.write(reply));
        return SelfCheckTest.map(reply.get("result"));
    }

    TrackSpec spec(String name) {
        return bitwig.state.tracks.stream().filter(t -> t.name.equals(name)).findFirst().orElseThrow();
    }

    /** What the extension asked of Bitwig, by name. */
    List<Object> calls() {
        return bitwig.calls.stream().map(c -> c.get(0)).collect(Collectors.toList());
    }

    /** The keys of the notes an answer lists (under `path`). */
    static List<Object> keys(Map<String, Object> result, Object... path) {
        return listAt(result, path).stream().map(n -> ((Number) at(n, "key")).intValue()).collect(Collectors.toList());
    }

    /** That Bitwig is as it was before: the transport's settings, the master, the tempo, the play start and the arms. */
    void assertPutBack(double playStart) {
        assertEquals(model.transport, bitwig.transportNow);
        assertFalse(bitwig.masterMuted, "the master isn't muted any more");
        assertEquals(model.tempo, bitwig.tempoNow);
        assertEquals(playStart, (Double) bitwig.playStart.v);
        assertFalse(bitwig.playingNow, "Bitwig is stopped");
        assertEquals(List.of(false, true, false, false), bitwig.state.tracks.stream().map(t -> t.arm).collect(Collectors.toList()),
                     "Keys is armed again, the others not");
        assertFalse(bitwig.calls.contains(List.of("stopWhileStopped")), "Stop was never pressed while Bitwig was stopped");
    }

    @Test
    @DisplayName("records a launcher clip into the arranger at bar 5, hears it in the replay, and puts everything back")
    void recordsALauncherClip() {
        studio();
        Map<String, Object> result = result(record(1, "rec", BAR_5, "pass", "A"));
        assertEquals("recorded", result.get("decided"), () -> Json.write(result));
        assertJson(obj("pass", "A", "decided", "recorded", "trackId", "rec", "track", "Rec", "slot", 0, "at", 16, "length", 8, "bar", 4, "record", true,
                       "arm", false, "tempo", obj("project", 120, "used", 120), "refused", list(), "heard", 6, "launcherClipDeleted", null,
                       "after", obj("checked", true, "recordOff", true, "playing", false, "playStart", 0, "restoreFailed", list()),
                       "diff", obj("tracksAdded", list(), "tracksRemoved", list(), "clipsChanged", list())),
                   SelfCheckTest.pick(result, "pass", "decided", "trackId", "track", "slot", "at", "length", "bar", "record", "arm", "tempo", "refused",
                                      "heard", "launcherClipDeleted", "after", "diff"));
        assertEquals(KEYS, keys(result, "played", "notes"));
        assertEquals(16.0, (Double) at(result, "played", "notes", 0, "at"), 0.1);
        assertEquals("done", at(result, "played", "ended"));
        assertEquals("bitwig", at(result, "played", "positionFrom"));
        assertEquals(true, at(result, "seen", "clipPlayed"));
        assertEquals(false, at(result, "seen", "automationWriteSeenOn"));
        double stopSent = (Double) at(result, "seen", "stopSentAt");
        assertTrue(stopSent >= 20 && stopSent < 21, "the track was stopped in the clip's last bar, at " + stopSent);
        // The replay heard the clip's notes in the arranger, with the launcher clip not playing.
        assertEquals(KEYS, keys(result, "replay", "notes"));
        assertJson(obj("from", 16, "to", 28, "inPass", 6, "before", 0, "launcherPlayed", false, "started", true, "ended", "done"),
                   SelfCheckTest.pick(SelfCheckTest.map(result.get("replay")), "from", "to", "inPass", "before", "launcherPlayed", "started", "ended"));

        assertEquals(List.of(new Recorded(spec("Rec"), BAR_5, BAR_5 + LENGTH, CLIP, BAR_5)), bitwig.state.recorded);
        assertEquals(List.of(CLIP), spec("Rec").slots, "the launcher keeps its clip");
        assertPutBack(0);

        // The track was stopped once; nothing but the launch and that stop happened while record was on.
        assertEquals(1, bitwig.calls.stream().filter(c -> c.get(0).equals("stopTrack")).count());
        int on = bitwig.calls.indexOf(List.of("transport", "isArrangerRecordEnabled", true));
        int off = bitwig.calls.indexOf(List.of("transport", "isArrangerRecordEnabled", false));
        assertTrue(on > 0 && off > on, Json.write(bitwig.calls));
        assertEquals(List.of(List.of("launch", 2, 0), List.of("stopTrack", 2)), bitwig.calls.subList(on + 1, off));
        // Record went off before Bitwig stopped, and the track went back to the arrangement before the replay played it (again
        // just before, as Bitwig once didn't take the first).
        assertEquals(List.of("stop", "returnToArrangement", "playStart", "returnToArrangement", "play", "stop"),
                     calls().subList(off + 1, calls().size()).stream().filter(c -> List.of("stop", "returnToArrangement", "playStart", "play").contains(c))
                            .collect(Collectors.toList()).subList(0, 6));
    }

    @Test
    @DisplayName("sets the transport up to record only the clip, silently, before record goes on")
    void setsUpBeforeRecording() {
        studio();
        app.send(record(1, "rec", BAR_5));
        // Up to the launch: everything set, and read back, before record goes on.
        bitwig.clock.advance(400);
        int on = bitwig.calls.indexOf(List.of("transport", "isArrangerRecordEnabled", true));
        assertTrue(on > 0, Json.write(bitwig.calls));
        assertEquals(List.of(List.of("transport", "isMetronomeEnabled", false), List.of("transport", "isArrangerLoopEnabled", false),
                             List.of("transport", "isArrangerOverdubEnabled", false), List.of("transport", "automationWriteMode", "touch"),
                             List.of("transport", "preRoll", "none"), List.of("transport", "defaultLaunchQuantization", "1"), List.of("masterMute", true),
                             List.of("playStart", BAR_5), List.of("arm", 1, false)),
                     bitwig.calls.subList(0, on), "only what isn't as recording wants it already");
        assertTrue(bitwig.masterMuted);
        assertEquals(false, bitwig.transportNow.get("isMetronomeEnabled"));
        bitwig.clock.advance(60000);
        assertEquals("recorded", at(app.take().get(0), "result", "decided"));
        assertPutBack(0);
    }

    @Test
    @DisplayName("plays the clip without recording it, to tell whether the link hears the track's notes")
    void playsWithoutRecording() {
        studio();
        Map<String, Object> heard = result(record(1, "rec", BAR_5, "record", false));
        assertEquals("heard", heard.get("decided"));
        assertEquals(KEYS, keys(heard, "played", "notes"));
        assertEquals(null, heard.get("replay"));
        assertEquals(List.of(), bitwig.state.recorded);
        assertFalse(bitwig.calls.stream().anyMatch(c -> c.equals(List.of("transport", "isArrangerRecordEnabled", true))), "record never went on");
        assertPutBack(0);

        studio(m -> m.notesHidden = true);
        Map<String, Object> deaf = result(record(1, "rec", BAR_5, "record", false));
        assertEquals("cantHear", deaf.get("decided"));
        assertEquals(0.0, deaf.get("heard"));
    }

    @Test
    @DisplayName("can't tell when it hears no notes at all, and says nothing was recorded when the replay hears none")
    void decidesFromWhatItHears() {
        studio(m -> m.notesHidden = true);
        Map<String, Object> deaf = result(record(1, "rec", BAR_5));
        assertEquals("cantTell", deaf.get("decided"));
        assertEquals(1, bitwig.state.recorded.size(), "Bitwig did record it: only a look tells");

        studio(m -> m.recordsNothing = true);
        Map<String, Object> nothing = result(record(1, "rec", BAR_5));
        assertEquals("nothing", nothing.get("decided"));
        assertEquals(6.0, nothing.get("heard"));
        assertEquals(0.0, at(nothing, "replay", "inPass"));
        assertPutBack(0);
    }

    @Test
    @DisplayName("records only an armed track when Bitwig needs that, and arms the one it records when asked")
    void armsWhenAsked() {
        studio(m -> m.recordsOnlyArmed = true);
        assertEquals("nothing", result(record(1, "rec", BAR_5)).get("decided"));
        assertPutBack(0);
        Map<String, Object> armed = result(record(2, "rec", 32, "arm", true));
        assertEquals("recorded", armed.get("decided"));
        assertEquals(true, armed.get("arm"));
        assertEquals(List.of(new Recorded(spec("Rec"), 32, 40, CLIP, 32)), bitwig.state.recorded, "Keys, disarmed, recorded nothing");
        assertPutBack(0);
    }

    @Test
    @DisplayName("records faster at a faster tempo, deletes the launcher clip, and replays from an earlier pass")
    void recordsFastAndDeletesTheLauncherClip() {
        studio();
        assertEquals("recorded", result(record(1, "rec", BAR_5)).get("decided"));
        Map<String, Object> fast = result(record(2, "rec", 48, "tempo", 240.0, "deleteLauncherClip", true, "replayFrom", BAR_5));
        assertEquals("recorded", fast.get("decided"), () -> Json.write(fast));
        assertJson(obj("project", 120, "used", 240), fast.get("tempo"));
        assertEquals(true, fast.get("launcherClipDeleted"));
        assertEquals(6.0, at(fast, "replay", "inPass"));
        assertEquals(6.0, at(fast, "replay", "before"), "the first pass's clip, heard on the way");
        assertEquals(false, at(fast, "replay", "launcherPlayed"));
        assertJson(list(obj("trackId", "rec", "slot", 0, "before", true, "after", false)), at(fast, "diff", "clipsChanged"));
        assertEquals(Arrays.asList((Object) null), spec("Rec").slots, "the launcher clip is gone");
        assertEquals(List.of(new Recorded(spec("Rec"), BAR_5, 24, CLIP, BAR_5), new Recorded(spec("Rec"), 48, 56, CLIP, 48)), bitwig.state.recorded);
        assertTrue(bitwig.calls.contains(Arrays.asList("tempo", 240.0)) && bitwig.calls.contains(Arrays.asList("tempo", 120.0)));
        assertPutBack(0);
        // Two seconds to record the two bars and eleven to replay bars 5 to 16 at 240 BPM: half what they'd take at 120.
        double ms = ((Number) fast.get("ms")).doubleValue();
        assertTrue(ms > 12000 && ms < 15000, "took " + ms + " ms");
    }

    @Test
    @DisplayName("keeps a trace of what it saw at each look and each command it sent, in order")
    void keepsATrace() {
        studio();
        Map<String, Object> result = result(record(1, "rec", BAR_5));
        List<?> trace = (List<?>) result.get("trace");
        List<String> whats = new ArrayList<>();
        String stop = null;
        for (Object line : trace) {
            String[] words = ((String) line).split(" ");
            String what = words[1].equals("end") ? "end " + words[2] : words[1];
            if (whats.isEmpty() || !whats.get(whats.size() - 1).equals(what)) whats.add(what);
            if (what.equals("stopTrack")) stop = (String) line;
        }
        assertEquals(List.of("recordOn", "launch", "look", "stopTrack", "look", "end done", "replay", "replayLook"), whats);
        // "<ms> stopTrack @<position> playing slot ...": the track was stopped in the clip's last bar, as it played.
        assertMatches("^\\d+ stopTrack @20\\.\\d{3} playing slot.*", stop);
    }

    @Test
    @DisplayName("records the whole clip when Bitwig's play position still says where the go before stopped as it starts")
    void waitsForThePositionToMove() {
        // The third live run: playing the clip without recording at bar 5 ended at beat 24.2, and recording it there next,
        // Bitwig still said 24.2 as it started, so the link stopped the track at once and only the first note went in.
        studio(m -> m.positionLag = 150);
        assertEquals("heard", result(record(1, "rec", BAR_5, "record", false)).get("decided"));
        bitwig.calls.clear();
        Map<String, Object> result = result(record(2, "rec", BAR_5));
        double stopSent = (Double) at(result, "seen", "stopSentAt");
        assertTrue(stopSent >= 20 && stopSent < 21, "the track was stopped in the clip's last bar, at " + stopSent + ": " + Json.write(result.get("trace")));
        assertEquals(KEYS, keys(result, "played", "notes"));
        assertEquals(List.of(new Recorded(spec("Rec"), BAR_5, BAR_5 + LENGTH, CLIP, BAR_5)), bitwig.state.recorded);
        assertEquals("recorded", result.get("decided"));
    }

    @Test
    @DisplayName("turns record off and stops just past the clip's end when Bitwig ignores stopping the track, then returns it to the arrangement for the replay")
    void endsWhenTheTrackDoesntStop() {
        studio(m -> m.trackStopIgnored = true);
        Map<String, Object> result = result(record(1, "rec", BAR_5));
        assertEquals("cut", at(result, "played", "ended"));
        int off = bitwig.calls.indexOf(List.of("transport", "isArrangerRecordEnabled", false));
        int stop = bitwig.calls.indexOf(List.of("stop"));
        assertTrue(off > 0 && stop > off, "record off first: " + Json.write(bitwig.calls));
        Recorded clip = bitwig.state.recorded.get(0);
        // 150 ms is 0.3 beats at 120 BPM, and the link looks every 25 ms.
        assertTrue(clip.end() > 24 && clip.end() < 24.45, "it ended just past the clip's end: " + clip);
        int play = bitwig.calls.indexOf(List.of("play"));
        assertTrue(play > 0 && bitwig.calls.subList(stop, play).stream().filter(c -> c.get(0).equals("returnToArrangement")).count() == 2,
                   "back to the arrangement after the recording and again before the replay: " + Json.write(bitwig.calls));
        assertEquals("recorded", result.get("decided"));
        assertPutBack(0);
    }

    @Test
    @DisplayName("stops when recordabort asks, which doesn't wait behind it, and puts everything back")
    void stopsWhenAborted() {
        studio();
        app.send(record(1, "rec", BAR_5));
        bitwig.clock.advance(1500);
        assertTrue(bitwig.playingNow && bitwig.recordOn());
        assertJson(obj("id", 2, "ok", true, "result", obj("running", true)), app.ask(obj("id", 2, "cmd", "recordabort")));
        bitwig.clock.advance(10000);
        Map<String, Object> result = SelfCheckTest.map(app.take().get(0).get("result"));
        assertEquals("aborted", result.get("decided"));
        assertEquals(null, result.get("replay"));
        assertEquals(list(), at(result, "after", "restoreFailed"));
        assertPutBack(0);
        assertJson(obj("id", 3, "ok", true, "result", obj("running", false)), app.ask(obj("id", 3, "cmd", "recordabort")));
    }

    /** Runs the stand-in's clock until the extension has asked Bitwig for `call`. */
    void runUntil(String call) {
        for (int i = 0; i < 60000 && !calls().contains(call); i++) bitwig.clock.advance(1);
        assertTrue(calls().contains(call), "it asked for " + call + ": " + Json.write(bitwig.calls));
    }

    @Test
    @DisplayName("presses Stop when aborted straight after the launch, before Bitwig shows it playing, and stays stopped")
    void stopsALaunchThatHasntShownYet() {
        studio(m -> m.startDelay = 200);
        app.send(record(1, "rec", BAR_5));
        runUntil("launch");
        assertFalse(bitwig.playingNow);
        app.ask(obj("id", 2, "cmd", "recordabort"));
        bitwig.clock.advance(10000);
        Map<String, Object> result = SelfCheckTest.map(app.take().get(0).get("result"));
        assertEquals("aborted", result.get("decided"));
        assertTrue(calls().lastIndexOf("stop") > calls().indexOf("launch"), Json.write(bitwig.calls));
        assertPutBack(0);
        bitwig.clock.advance(10000);
        assertFalse(bitwig.playingNow, "Bitwig never started afterwards");
    }

    @Test
    @DisplayName("stops the replay when changes are turned off, and says so")
    void stopsTheReplayWhenChangesGoOff() {
        studio(m -> m.startDelay = 200);
        app.send(record(1, "rec", BAR_5));
        runUntil("play");
        bitwig.allowChanges(false);
        bitwig.clock.advance(15000);
        Map<String, Object> result = SelfCheckTest.map(app.take().get(0).get("result"));
        assertEquals("aborted", result.get("decided"));
        assertEquals("Changes were turned off.", result.get("abortWhy"));
        assertEquals("aborted", at(result, "replay", "ended"));
        assertPutBack(0);
        bitwig.clock.advance(10000);
        assertFalse(bitwig.playingNow, "Bitwig never started afterwards");
    }

    @Test
    @DisplayName("neither turns record on nor launches once changes are turned off while it sets Bitwig up")
    void doesntStartOnceChangesGoOff() {
        studio();
        app.send(record(1, "rec", BAR_5));
        bitwig.allowChanges(false);
        bitwig.clock.advance(10000);
        Map<String, Object> result = SelfCheckTest.map(app.take().get(0).get("result"));
        assertEquals("aborted", result.get("decided"));
        assertFalse(calls().contains("launch"), Json.write(bitwig.calls));
        assertFalse(bitwig.calls.contains(List.of("transport", "isArrangerRecordEnabled", true)), Json.write(bitwig.calls));
        assertPutBack(0);
    }

    @Test
    @DisplayName("turns record off, stops Bitwig and puts everything back when Bitwig stops the link part way")
    void stopsWithTheLink() {
        studio();
        app.send(record(1, "rec", BAR_5));
        bitwig.clock.advance(1500);
        assertTrue(bitwig.playingNow && bitwig.recordOn() && bitwig.masterMuted);
        bitwig.link.exit();
        bitwig.clock.advance(1000);
        assertFalse(bitwig.recordOn());
        assertPutBack(0);
        assertEquals(List.of(), app.take(), "the app's connection has closed");
    }

    @Test
    @DisplayName("refuses, putting everything back, when a setting doesn't take")
    void refusesWhenASettingDoesntTake() {
        studio(m -> m.settingsIgnored.add("preRoll"));
        Map<String, Object> result = result(record(1, "rec", BAR_5));
        assertEquals("refused", result.get("decided"));
        assertEquals(list("pre-roll"), result.get("refused"));
        assertFalse(calls().contains("launch"));
        assertFalse(bitwig.calls.contains(List.of("transport", "isArrangerRecordEnabled", true)));
        assertEquals(List.of(), bitwig.state.recorded);
        assertPutBack(0);

        studio(m -> m.settingsIgnored.add("isArrangerRecordEnabled"));
        Map<String, Object> noRecord = result(record(1, "rec", BAR_5));
        assertEquals(list("arranger record"), noRecord.get("refused"));
        assertFalse(calls().contains("launch"));
        assertPutBack(0);
    }

    @Test
    @DisplayName("stops touching Bitwig once another project comes to the front")
    void abandonsAnotherProject() {
        studio();
        int[] from = new int[1];
        bitwig.during("record", () -> bitwig.playingNow, () -> {
            bitwig.projectName.v = "My Song";
            from[0] = bitwig.calls.size();
        });
        Map<String, Object> result = result(record(1, "rec", BAR_5));
        assertEquals("abandoned", result.get("decided"));
        assertJson(obj("checked", false, "restoreFailed", null), result.get("after"));
        assertEquals(null, result.get("diff"));
        assertTrue(from[0] > 0);
        assertEquals(List.of(), bitwig.calls.subList(from[0], bitwig.calls.size()), "nothing more asked of Bitwig");
    }

    @Test
    @DisplayName("finds its track by its ID when a track goes in above it, and says the track is new")
    void findsItsTrackWhenOneGoesInAbove() {
        studio();
        bitwig.during("record", () -> bitwig.playingNow, () -> bitwig.state.tracks.add(0, new TrackSpec("Rec (pasted)", "Instrument").id("pasted").slots(CLIP)));
        Map<String, Object> result = result(record(1, "rec", BAR_5));
        assertEquals("recorded", result.get("decided"));
        assertEquals(List.of("stopTrack", 3), bitwig.calls.stream().filter(c -> c.get(0).equals("stopTrack")).findFirst().orElseThrow());
        assertEquals(List.of("returnToArrangement", 3), bitwig.calls.stream().filter(c -> c.get(0).equals("returnToArrangement")).findFirst().orElseThrow());
        assertEquals(spec("Rec"), bitwig.state.recorded.get(0).track());
        assertJson(obj("tracksAdded", list(obj("id", "pasted", "name", "Rec (pasted)")), "tracksRemoved", list(), "clipsChanged", list()), result.get("diff"));
    }

    @Test
    @DisplayName("says when Bitwig wrote automation as it recorded, and goes by the clock when Bitwig's position doesn't move")
    void notesAutomationAndTheClock() {
        studio(m -> {
            m.automationWhileRecording = true;
            m.positionStill = true;
        });
        Map<String, Object> result = result(record(1, "rec", BAR_5));
        assertEquals(true, at(result, "seen", "automationWriteSeenOn"));
        assertEquals("clock", at(result, "played", "positionFrom"));
        assertEquals("recorded", result.get("decided"));
        assertEquals(false, bitwig.transportNow.get("isArrangerAutomationWriteEnabled"));
        assertPutBack(0);
    }

    @Test
    @DisplayName("refuses an odd request, while Bitwig plays or records, and keeps to the project it names")
    void refusesOddRequests() {
        studio();
        assertEquals("There's no track with ID gone in the project listing.", error(app.ask(record(1, "gone", BAR_5))));
        assertEquals("Say which track to record by its ID (trackId).", error(app.ask(record(2, null, BAR_5))));
        assertMatches("^at, length and replayFrom have to be whole bars: 4 beats each in 4/4\\.$", error(app.ask(record(3, "rec", 17.0))));
        assertMatches("whole bars", error(app.ask(record(4, "rec", BAR_5, "length", 6.0))));
        assertMatches("length is a number of beats, more than 0", error(app.ask(record(5, "rec", BAR_5, "length", 0.0))));
        assertEquals("The replay can't start (replayFrom) after where the clip is recorded (at).", error(app.ask(record(6, "rec", BAR_5, "replayFrom", 32.0))));
        assertMatches("^The tempo is from 20 to 666 BPM", error(app.ask(record(7, "rec", BAR_5, "tempo", 900.0))));
        assertEquals("Slot 0 on that track has no clip to record.", error(app.ask(record(8, "keys", BAR_5))));
        assertMatches("record is true or false", error(app.ask(record(9, "rec", BAR_5, "record", "yes"))));
        Map<String, Object> elsewhere = record(10, "rec", BAR_5);
        elsewhere.put("onlyIn", "Night Drive");
        assertEquals(LinkTest.elsewhere("MO Live Test", "Night Drive"), error(app.ask(elsewhere)));
        bitwig.allowChanges(false);
        assertMatches(LinkTest.CHANGES_OFF, error(app.ask(record(11, "rec", BAR_5))));
        bitwig.allowChanges(true);
        assertEquals(List.of(), bitwig.calls);

        bitwig.transportNow.put("isArrangerRecordEnabled", true);
        bitwig.refresh();
        assertEquals("Bitwig's arranger record is on. Turn it off, then ask again.", error(app.ask(record(12, "rec", BAR_5))));
        bitwig.transportNow.put("isArrangerRecordEnabled", false);
        bitwig.playingNow = true;
        bitwig.refresh();
        assertEquals("Bitwig is playing. Stop it, then ask again.", error(app.ask(record(13, "rec", BAR_5))));
        bitwig.playingNow = false;
        bitwig.refresh();
        assertEquals(List.of(), bitwig.calls);
    }

    @Test
    @DisplayName("says a new track's ID, and puts a clip on a track found by its ID")
    void newTracksAndClipsById() {
        studio();
        Map<String, Object> made = SelfCheckTest.map(app.waitFor(obj("id", 1, "cmd", "newtrack", "name", "Rec 3.9")).get("result"));
        assertJson(obj("index", 3, "name", "Rec 3.9", "id", "new-1"), made);
        // A track going in above it meanwhile doesn't put the clip on the wrong one.
        bitwig.state.tracks.add(0, new TrackSpec("Pasted", "Instrument").id("pasted"));
        bitwig.refresh();
        assertJson(obj("slot", 0), app.waitFor(obj("id", 2, "cmd", "clip", "trackId", "new-1", "slot", 0, "path", CLIP)).get("result"));
        assertEquals(List.of(CLIP), spec("Rec 3.9").slots);
        assertEquals(List.of(), spec("Pasted").slots);
        assertEquals("There's no track with ID nope in the project listing.",
                     error(app.ask(obj("id", 3, "cmd", "clip", "trackId", "nope", "slot", 1, "path", CLIP))));
    }

    @Test
    @DisplayName("probes whether Bitwig shows and hides its panels for the link, and shows them as they were afterwards")
    void probesThePanels() {
        studio();
        Map<String, Object> result = result(obj("id", 1, "cmd", "probe", "onlyIn", "MO Live Test", "via", "direct", "gap", 100.0));
        assertJson(obj("via", "direct", "layout", "ARRANGE", "steps", list(
                       obj("action", Link.TOGGLE_ARRANGER, "ran", list(true, true), "visible", list(true, false, true), "restored", null),
                       obj("action", Link.FOCUS_LAUNCHER, "ran", list(true, true), "visible", list(true, true, false), "restored", true),
                       obj("action", Link.FOCUS_ARRANGER, "ran", list(true, true), "visible", list(true, true, false), "restored", true))),
                   result);

        // A Bitwig that carries out its panels' commands only for hardware: nothing changes, so nothing is restored.
        studio(m -> m.onlyHardware(false));
        Map<String, Object> ignored = result(obj("id", 1, "cmd", "probe", "onlyIn", "MO Live Test", "gap", 100.0));
        assertEquals("flush", ignored.get("via"));
        for (Object step : listAt(ignored, "steps")) {
            assertJson(list(true, true, true), at(step, "visible"));
            assertEquals(null, at(step, "restored"));
        }

        studio(m -> m.actions.removeIf(a -> a.id().equals(Link.TOGGLE_ARRANGER)));
        Map<String, Object> missing = result(obj("id", 1, "cmd", "probe", "via", "direct", "gap", 0.0));
        assertJson(obj("action", Link.TOGGLE_ARRANGER, "missing", true), at(missing, "steps", 0));
        assertEquals(3, listAt(missing, "steps").size());
        assertMatches("gap is from 0 to 10000 ms", error(app.ask(obj("id", 2, "cmd", "probe", "gap", -1.0))));
    }

    @Test
    @DisplayName("says in its self-check how Bitwig is set for recording")
    void selfChecksRecording() {
        studio();
        Map<String, Object> result = SelfCheckTest.map(app.ask(obj("id", 1, "cmd", "selftest")).get("result"));
        assertEquals("Arranger record is off, pre-roll one_bar, launch quantize 1/4, automation write mode latch; the master track is not muted.",
                     SelfCheckTest.detail(result, "recording into the arranger"));
    }
}
