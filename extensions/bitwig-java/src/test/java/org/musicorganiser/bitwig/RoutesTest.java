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
import org.musicorganiser.bitwig.FakeBitwig.Context;
import org.musicorganiser.bitwig.FakeBitwig.Model;

/**
 * The routes (extension 3.7): Bitwig 6.1.3 ignored every command of its user interface the link ran (Save, copy, paste,
 * bringing a panel to the front), wherever the link ran it from, while the same Save worked from a controller's hardware
 * button. So the link can run such a command by a route: straight away ("direct"), from Bitwig's flush ("flush"), or
 * through a MIDI loopback, as a SysEx message it sent itself ("midi") or a press of one of its own buttons, whose command
 * Bitwig runs ("button") or the link runs ("buttonCode"). The stand-in Bitwig keeps track of where each call into the
 * extension comes from, and the model says where Bitwig carries the commands out, so these check each route does what it
 * says, says when it didn't come back, and never runs anything that comes back too late.
 */
class RoutesTest {
    /** Every route, in the order the link lists them. */
    static final List<String> ROUTES = List.of("direct", "flush", "midi", "button", "buttonCode");
    /** The routes through the loopback. */
    static final List<String> LOOPBACK = List.of("midi", "button", "buttonCode");

    @TempDir
    Path reports;

    FakeBitwig bitwig;
    App app;

    /**
     * A test project with unsaved changes, a clip in the launcher on two tracks, Bitwig's Save, Copy, Paste and focus
     * actions, and changes allowed. The loopback works unless `extra` says otherwise.
     */
    void studio(Consumer<Model> extra) {
        Model m = new Model();
        m.project = "MO Live Test";
        m.modified = true;
        m.selectedTrack = "Bass";
        m.tracks(
            track("Bass", "Instrument").slots("C:\\MIDI\\Bass.mid"),
            track("Lead", "Instrument").slots("C:\\MIDI\\Lead.mid", null),
            track("Master", "Master"));
        m.actions.add(action("Save", "Save", "File"));
        m.actions.add(action("Copy", "Copy", "Edit"));
        m.actions.add(action("Paste", "Paste", "Edit"));
        m.actions.add(action(Link.FOCUS_ARRANGER, "Focus/toggle Arranger Timeline Panel", "View"));
        m.actions.add(action(Link.FOCUS_LAUNCHER, "Focus/toggle Clip Launcher Panel", "View"));
        m.loopback = true;
        extra.accept(m);
        bitwig = FakeBitwig.start(m, reports);
        bitwig.allowChanges(true);
        app = bitwig.connect();
        app.take();
    }

    void studio() {
        studio(m -> {});
    }

    static Map<String, Object> save(Object id, Object via) {
        return obj("id", id, "cmd", "action", "action", "Save", "via", via);
    }

    /** Runs Save by a route on a project with unsaved changes, and returns the answer. */
    Map<String, Object> saveVia(String via) {
        bitwig.modified.v = true;
        return app.waitFor(save(1, via));
    }

    /** How many times the extension ran Bitwig's action `id` (or Bitwig ran it for one of the extension's buttons). */
    long invoked(String id) {
        return bitwig.calls.stream().filter(c -> c.equals(List.of("invoke", id))).count();
    }

    @Test
    @DisplayName("runs Save by every route, and says it ran")
    void runsSaveByEveryRoute() {
        studio();
        for (String via : ROUTES) {
            bitwig.calls.clear();
            assertJson(obj("id", 1, "ok", true, "result", obj("project", "MO Live Test", "via", via, "ran", true)), saveVia(via), via);
            assertEquals(false, bitwig.modified.v, via + " saved the project");
            assertEquals(1, invoked("Save"), via);
        }
        assertEquals(List.of(), bitwig.ignored);
    }

    @Test
    @DisplayName("sends its own SysEx message for the midi route, and notes on channel 16 for the button routes")
    void sendsItsOwnMidi() {
        studio();
        // A SysEx message when it started and when the app connected, to learn whether the loopback works.
        assertEquals(List.of(Arrays.asList("sysex", Link.message(1)), Arrays.asList("sysex", Link.message(2))), bitwig.midi);
        assertEquals("F0 7D 4D 4F 42 00 00 01 F7", Link.message(1));
        // The message's number, seven bits to a byte.
        assertEquals("F0 7D 4D 4F 42 01 7F 7F F7", Link.message(0x7FFF));
        bitwig.midi.clear();
        saveVia("midi");
        saveVia("button");
        saveVia("buttonCode");
        // Save is the first command the link has buttons for: note 100 for "button", 110 for "buttonCode", pressed and let go.
        assertEquals(List.of(Arrays.asList("sysex", Link.message(3)),
                             Arrays.asList("midi", 0x9F, 100, 127), Arrays.asList("midi", 0x8F, 100, 0),
                             Arrays.asList("midi", 0x9F, 110, 127), Arrays.asList("midi", 0x8F, 110, 0)),
                     bitwig.midi);
        // Paste is the third.
        bitwig.midi.clear();
        app.waitFor(obj("id", 2, "cmd", "action", "action", "Paste", "via", "buttonCode"));
        assertEquals(List.of(Arrays.asList("midi", 0x9F, 112, 127), Arrays.asList("midi", 0x8F, 112, 0)), bitwig.midi);
    }

    @Test
    @DisplayName("says a route didn't run when nothing comes back, after about a second, and runs nothing")
    void saysWhenNothingCameBack() {
        studio(m -> {
            m.loopback = false;
            m.noFlush = true;
        });
        for (String via : List.of("flush", "midi", "button", "buttonCode")) {
            bitwig.modified.v = true;
            app.send(save(1, via));
            bitwig.clock.advance(Link.LOOPBACK_WAIT - 50);
            assertEquals(List.of(), app.take(), via + " waits a second for the command to come back");
            bitwig.clock.advance(100);
            assertJson(list(obj("id", 1, "ok", true, "result", obj("project", "MO Live Test", "via", via, "ran", false))), app.take());
            bitwig.clock.advance(10000);
            assertEquals(true, bitwig.modified.v, via);
        }
        assertEquals(List.of(), bitwig.calls);
    }

    @Test
    @DisplayName("ignores its own messages and presses that come back after it gave up on them")
    void ignoresWhatComesBackLate() {
        // Through a loopback that takes longer than the link waits.
        studio(m -> m.loopbackDelay = Link.LOOPBACK_WAIT + 500);
        for (String via : List.of("midi", "buttonCode")) {
            assertJson(obj("project", "MO Live Test", "via", via, "ran", false), saveVia(via).get("result"));
            assertEquals(true, bitwig.modified.v, via);
        }
        assertEquals(List.of(), bitwig.calls, "nothing ran when the message and the press came back late");

        // A message that comes back late while the link waits for a newer one doesn't run the newer one: the link goes by the
        // number in the message.
        app.send(save(2, "midi"));   // back after LOOPBACK_WAIT + 500, once the link has given up on it
        bitwig.clock.advance(Link.LOOPBACK_WAIT + 50);
        assertJson(list(obj("id", 2, "ok", true, "result", obj("project", "MO Live Test", "via", "midi", "ran", false))), app.take());
        bitwig.model.loopbackDelay = 5000;
        app.send(save(3, "midi"));   // still on its way when message 2 comes back
        bitwig.clock.advance(600);
        assertEquals(0, invoked("Save"), "message 2 came back while the link waited for message 3, and didn't run Save");
        bitwig.clock.advance(Link.LOOPBACK_WAIT);
        assertJson(list(obj("id", 3, "ok", true, "result", obj("project", "MO Live Test", "via", "midi", "ran", false))), app.take());
        bitwig.clock.advance(10000);
        assertEquals(0, invoked("Save"));

        // Nor does the message the link sends itself as an app connects, to learn whether the loopback works.
        app.send(save(4, "midi"));   // back after 5 s
        bitwig.model.loopbackDelay = 5;
        bitwig.midi.clear();
        App other = bitwig.connect();
        assertEquals("hello", other.take().get(0).get("event"));
        bitwig.clock.advance(100);
        assertEquals(1, bitwig.midi.stream().filter(m -> m.get(0).equals("sysex")).count(), "the connect sent a message of its own");
        assertEquals(0, invoked("Save"), "the connect's message came back while the link waited for message 4, and didn't run Save");
        bitwig.clock.advance(Link.LOOPBACK_WAIT);
        assertJson(list(obj("id", 4, "ok", true, "result", obj("project", "MO Live Test", "via", "midi", "ran", false))), app.take());
        bitwig.clock.advance(10000);
        assertEquals(0, invoked("Save"));

        // Its own message back in time, the link runs the command it waits for.
        app.send(save(5, "midi"));
        bitwig.clock.advance(400);
        assertJson(list(obj("id", 5, "ok", true, "result", obj("project", "MO Live Test", "via", "midi", "ran", true))), app.take());
        bitwig.clock.advance(10000);
        assertEquals(1, invoked("Save"));

        // A message with a number the link never waited for, someone else's SysEx, and one that isn't whole: nothing runs.
        bitwig.calls.clear();
        bitwig.sysexIn("f07d4d4f42010101f7");
        bitwig.sysexIn("f07e7f0601f7");
        bitwig.sysexIn("f07d4d4f4201f7");
        bitwig.sysexIn("f07d4d4f42ff0101f7");
        bitwig.clock.advance(1000);
        assertEquals(List.of(), bitwig.calls);
    }

    @Test
    @DisplayName("saves by its buttons when Bitwig carries out its commands only for hardware, and says the other routes ran in vain")
    void savesByItsButtonsWhenOnlyHardwareCounts() {
        studio(m -> m.onlyHardware(false));
        for (String via : List.of("direct", "flush", "midi")) {
            assertJson(obj("project", "MO Live Test", "via", via, "ran", true), saveVia(via).get("result"));
            assertEquals(true, bitwig.modified.v, via + " left the project unsaved");
        }
        assertEquals(List.of("Save in SOCKET", "Save in FLUSH", "Save in SYSEX"), bitwig.ignored);
        for (String via : List.of("button", "buttonCode")) {
            assertJson(obj("project", "MO Live Test", "via", via, "ran", true), saveVia(via).get("result"));
            assertEquals(false, bitwig.modified.v, via + " saved the project");
        }
        assertEquals(3, bitwig.ignored.size());
    }

    @Test
    @DisplayName("saves by its own SysEx message when Bitwig counts that as hardware too")
    void savesByMidiWhenSysexCounts() {
        studio(m -> m.onlyHardware(true));
        assertJson(obj("project", "MO Live Test", "via", "direct", "ran", true), saveVia("direct").get("result"));
        assertEquals(true, bitwig.modified.v);
        for (String via : LOOPBACK) {
            assertJson(obj("project", "MO Live Test", "via", via, "ran", true), saveVia(via).get("result"));
            assertEquals(false, bitwig.modified.v, via + " saved the project");
        }
        assertEquals(List.of("Save in SOCKET"), bitwig.ignored);
    }

    @Test
    @DisplayName("runs a command by a route from the context it's named for")
    void runsFromTheRightContext() {
        studio();
        List<Context> seen = new ArrayList<>();
        Map<String, Context> expected = Map.of("direct", Context.SOCKET, "flush", Context.FLUSH, "midi", Context.SYSEX,
                                               "button", Context.HARDWARE, "buttonCode", Context.HARDWARE);
        for (String via : ROUTES) {
            bitwig.model.honoured.clear();
            saveVia(via);
            bitwig.model.honoured.addAll(List.of(Context.values()));
            seen.add(Context.valueOf(bitwig.ignored.get(bitwig.ignored.size() - 1).replace("Save in ", "")));
        }
        assertEquals(ROUTES.stream().map(expected::get).collect(Collectors.toList()), seen);
    }

    @Test
    @DisplayName("runs a command from Bitwig's flush when the request names no route (extension 3.8), as Save worked from there")
    void runsFromTheFlushByDefault() {
        // Bitwig 6.1.3 saved from the flush, and ignored Save run straight away.
        studio(m -> m.honoured.remove(Context.SOCKET));
        bitwig.modified.v = true;
        assertJson(obj("id", 1, "ok", true, "result", obj("project", "MO Live Test", "via", "flush", "ran", true)), app.waitFor(save(1, null)));
        assertEquals(false, bitwig.modified.v);
        assertEquals(List.of(), bitwig.ignored);

        // Arranging too: its copy and paste run from the flush.
        studio(m -> m.honoured.remove(Context.TASK));
        Map<String, Object> request = arrange(2, "direct");
        request.remove("via");
        Map<String, Object> result = SelfCheckTest.map(app.waitFor(request).get("result"));
        assertJson(obj("arranged", true, "via", "flush"), SelfCheckTest.pick(result, "arranged", "via"));
        assertEquals(List.of(), bitwig.ignored);
    }

    @Test
    @DisplayName("refuses a route it doesn't know, and an action Bitwig doesn't have, before running anything")
    void refusesAnUnknownRoute() {
        studio(m -> m.actions.add(action("toggle_metronome", "Toggle Metronome", "Transport")));
        bitwig.midi.clear();
        for (Object via : list("nope", "Direct", "", 5, true, list("midi"), obj()))
            assertMatches("^The route \\(via\\) has to be one of direct, flush, midi, button, buttonCode, not ",
                          error(app.waitFor(save(1, via))));
        assertEquals("The route (via) has to be one of direct, flush, midi, button, buttonCode, not nope.", error(app.waitFor(save(1, "nope"))));
        assertEquals("Bitwig has no action Format C:.",
                     error(app.waitFor(obj("id", 2, "cmd", "action", "action", "Format C:", "via", "button"))));
        assertEquals(List.of(), bitwig.calls);
        assertEquals(List.of(), bitwig.midi);

        // Bitwig has an action the link has no buttons for: the buttons are for Save, Copy, Paste and the focus actions.
        assertEquals("The link has buttons for Save, Copy, Paste, focus_or_toggle_arranger, focus_or_toggle_clip_launcher, not "
                     + "toggle_metronome, so it can't run that via button.",
                     error(app.waitFor(obj("id", 3, "cmd", "action", "action", "toggle_metronome", "via", "button"))));
        assertEquals(List.of(), bitwig.midi);
    }

    @Test
    @DisplayName("still needs changes turned on, and keeps to the project the request names, by any route")
    void needsChangesAndKeepsToTheProject() {
        studio();
        bitwig.midi.clear();
        bitwig.allowChanges(false);
        for (String via : ROUTES) assertMatches(LinkTest.CHANGES_OFF, error(app.waitFor(save(1, via))));
        bitwig.allowChanges(true);
        for (String via : ROUTES) {
            Map<String, Object> request = save(2, via);
            request.put("onlyIn", "Night Drive");
            assertEquals(LinkTest.elsewhere("MO Live Test", "Night Drive"), error(app.waitFor(request)));
        }
        assertEquals(List.of(), bitwig.calls);
        assertEquals(List.of(), bitwig.midi);

        // Another project comes to the front while the command is on its way back: the link doesn't run it there.
        for (String via : List.of("flush", "midi", "buttonCode")) {
            bitwig.projectName.v = "MO Live Test";
            bitwig.switchDuring("action", "My Song");
            Map<String, Object> request = save(3, via);
            request.put("onlyIn", "MO Live Test");
            assertEquals(LinkTest.elsewhere("My Song", "MO Live Test"), error(app.waitFor(request)), via);
        }
        assertEquals(List.of(), bitwig.calls);
    }

    @Test
    @DisplayName("says in hello whether its own MIDI has come back through the loopback")
    void saysWhetherTheLoopbackWorks() {
        studio(m -> m.loopback = false);
        assertEquals(false, at(app.ask(obj("id", 1, "cmd", "hello")), "result", "loopback"));

        studio();
        // The message it sent as it started hasn't come back yet.
        assertEquals(false, at(app.ask(obj("id", 1, "cmd", "hello")), "result", "loopback"));
        bitwig.clock.advance(10);
        assertEquals(true, at(app.ask(obj("id", 2, "cmd", "hello")), "result", "loopback"));
        // Each app that connects is told in its hello.
        App second = bitwig.connect();
        assertEquals(true, at(second.take().get(0), "result", "loopback"));
    }

    @Test
    @DisplayName("says in its self-check which buttons it has and whether the loopback works")
    void checksItsRoutes() {
        studio(m -> m.loopback = false);
        Map<String, Object> result = SelfCheckTest.map(app.ask(obj("id", 1, "cmd", "selftest")).get("result"));
        assertEquals(true, result.get("ok"));
        assertEquals("Buttons for Save, Copy, Paste, focus_or_toggle_arranger, focus_or_toggle_clip_launcher on MIDI channel 16 (notes from "
                     + "100 and from 110). The link hasn't had its own MIDI back, so its MIDI in and out may not be set (or their port isn't "
                     + "working), and the routes through MIDI need them. Bitwig 6.1.3 has only saved for the link once they were set. "
                     + Link.LOOPBACK_SETUP,
                     SelfCheckTest.detail(result, "command routes"));
        assertMatches("set both MIDI in and MIDI out to \"Music Organiser Bitwig\" \\(the loopMIDI port\\), never \"Music Organiser\"",
                      Link.LOOPBACK_SETUP);
        assertEquals(10, bitwig.buttons.size());
        assertEquals(List.of(60.0, 20.0), List.of(bitwig.surfaceSize[0], bitwig.surfaceSize[1]));

        studio();
        bitwig.clock.advance(10);
        result = SelfCheckTest.map(app.ask(obj("id", 1, "cmd", "selftest")).get("result"));
        assertMatches("The link has had its own MIDI back, so the loopback works\\.$", SelfCheckTest.detail(result, "command routes"));

        // Without Bitwig's Copy and Paste actions, the link has buttons for the rest.
        studio(m -> m.actions.removeIf(a -> a.id().equals("Copy") || a.id().equals("Paste")));
        result = SelfCheckTest.map(app.ask(obj("id", 1, "cmd", "selftest")).get("result"));
        assertMatches("^Buttons for Save, focus_or_toggle_arranger, focus_or_toggle_clip_launcher on MIDI channel 16 ",
                      SelfCheckTest.detail(result, "command routes"));
        assertEquals(6, bitwig.buttons.size());
    }

    @Test
    @DisplayName("still runs commands straight away and from the flush when Bitwig won't give it MIDI ports")
    void startsWithoutMidiPorts() {
        studio(m -> m.broken.add("getMidiInPort"));
        assertEquals(false, at(app.ask(obj("id", 1, "cmd", "hello")), "result", "loopback"));
        for (String via : List.of("direct", "flush"))
            assertJson(obj("project", "MO Live Test", "via", via, "ran", true), saveVia(via).get("result"));
        for (String via : LOOPBACK)
            assertEquals("This Bitwig didn't let the link set up its MIDI ports and buttons, so it can't run a command via " + via
                         + ". A selftest says why.", error(saveVia(via)));
        Map<String, Object> result = SelfCheckTest.map(app.ask(obj("id", 2, "cmd", "selftest")).get("result"));
        assertEquals(List.of("command routes"), listAt(result, "checks").stream().filter(c -> !(Boolean) at(c, "ok")).map(c -> at(c, "name"))
                                                                      .collect(Collectors.toList()));
        assertMatches("^Couldn't set it up: java.lang.AbstractMethodError: ", SelfCheckTest.detail(result, "command routes"));
        assertEquals(List.of(), bitwig.midi);
    }

    // ---- Arrange by a route ----

    static Map<String, Object> arrange(Object id, Object via) {
        return obj("id", id, "cmd", "arrange", "track", 0, "slot", 0, "at", ArrangerTest.BAR_5, "via", via);
    }

    @Test
    @DisplayName("arranges a launcher clip by its buttons when Bitwig carries out its commands only for hardware")
    void arrangesByItsButtons() {
        for (String via : List.of("button", "buttonCode")) {
            studio(m -> m.onlyHardware(false));
            Map<String, Object> result = SelfCheckTest.map(app.waitFor(arrange(1, via)).get("result"));
            assertJson(obj("arranged", true, "appeared", true, "sameAsSource", true, "launcherFocus", 1, "focus", 1, "via", via,
                           "ran", ArrangerTest.ran(true, true, true, true)),
                       SelfCheckTest.pick(result, "arranged", "appeared", "sameAsSource", "launcherFocus", "focus", "via", "ran"), via);
            assertEquals(1, bitwig.state.arranged.size(), via);
            assertEquals(List.of(), bitwig.ignored, via);
            // Bitwig's own Copy and Paste actions, not the API's copy and paste.
            assertEquals(List.of(List.of("invoke", Link.FOCUS_LAUNCHER), List.of("invoke", "Copy"), List.of("invoke", Link.FOCUS_ARRANGER),
                                 List.of("invoke", "Paste")),
                         bitwig.calls.stream().filter(c -> c.get(0).equals("invoke") || c.get(0).equals("copy") || c.get(0).equals("paste"))
                                     .collect(Collectors.toList()), via);
        }
    }

    @Test
    @DisplayName("doesn't arrange straight away when Bitwig carries out its commands only for hardware, and says what it saw")
    void doesntArrangeDirectlyWhenOnlyHardwareCounts() {
        studio(m -> m.onlyHardware(false));
        Map<String, Object> result = SelfCheckTest.map(app.waitFor(arrange(1, "direct")).get("result"));
        assertJson(obj("arranged", false, "appeared", false, "via", "direct", "ran", ArrangerTest.ran(true, true, true, true)),
                   SelfCheckTest.pick(result, "arranged", "appeared", "via", "ran"));
        assertEquals(List.of(), bitwig.state.arranged);
        assertEquals(List.of(Link.FOCUS_LAUNCHER + " in TASK", "copy in TASK", Link.FOCUS_ARRANGER + " in TASK", "paste in TASK"),
                     bitwig.ignored);

        // Through the midi route, it's the same unless Bitwig counts a SysEx message as hardware.
        studio(m -> m.onlyHardware(false));
        result = SelfCheckTest.map(app.waitFor(arrange(1, "midi")).get("result"));
        assertEquals(false, result.get("arranged"));
        studio(m -> m.onlyHardware(true));
        result = SelfCheckTest.map(app.waitFor(arrange(1, "midi")).get("result"));
        assertEquals(true, result.get("arranged"));
    }

    @Test
    @DisplayName("doesn't paste when the copy didn't run by its route, and doesn't fail")
    void doesntPasteWhenTheCopyDidntRun() {
        studio(m -> m.loopback = false);
        Map<String, Object> reply = app.waitFor(arrange(1, "button"));
        assertEquals(true, reply.get("ok"));
        Map<String, Object> result = SelfCheckTest.map(reply.get("result"));
        assertJson(obj("arranged", false, "appeared", false, "launcherFocus", 0, "focus", 0, "beforePaste", ArrangerTest.clip(null),
                       "via", "button", "ran", ArrangerTest.ran(false, false, null, null)),
                   SelfCheckTest.pick(result, "arranged", "appeared", "launcherFocus", "focus", "beforePaste", "via", "ran"));
        // The play start stays where it was.
        assertFalse(bitwig.calls.stream().anyMatch(c -> c.get(0).equals("playStart")));
        assertEquals(List.of(), bitwig.state.arranged);
    }

    @Test
    @DisplayName("stops arrange's checks before their paste and Duplicate when the launcher's focus action doesn't come back (extension 3.8)")
    void stopsChecksWithoutTheLaunchersFocus() {
        // The timeline's focus action is ignored, so arrange pastes into the launcher; then the loopback goes, so the
        // checks' focus actions never run, and neither does anything after them.
        studio(m -> {
            m.actions.add(action("Duplicate", "Duplicate", "Editing"));
            m.focusIgnored.add(Link.FOCUS_ARRANGER);
            m.loopbackCut = () -> bitwig.calls.contains(List.of("invoke", "Paste"));
        });
        Map<String, Object> request = arrange(1, "midi");
        request.put("checks", true);
        Map<String, Object> result = SelfCheckTest.map(app.waitFor(request, 30000).get("result"));
        assertEquals(2.0, result.get("launcherClipsAfter"));
        assertJson(obj("launcherPaste", obj("slot", 2, "selected", true, "focus", 0, "ran", obj("focus", false, "paste", null), "made", false,
                                            "colour", null, "sameAsMark", false, "removed", false),
                       "duplicate", obj("selected", true, "focus", 0, "ran", obj("focus", false, "duplicate", null), "launcher", 0, "tracks", 0,
                                        "removed", false)),
                   result.get("checks"));
        assertEquals(1, invoked("Paste"));
        assertEquals(0, invoked("Duplicate"));
    }

    @Test
    @DisplayName("stops a check before its paste when the launcher's focus action hid the launcher and didn't come back to show it (extension 3.8)")
    void stopsAChecksPasteWhenTheLauncherStaysHidden() {
        // Arrange pastes into the launcher, which is still in front for the first check: its focus action hides it, and
        // the run that would bring it back never comes back, nor does anything after it.
        studio(m -> {
            m.actions.add(action("Duplicate", "Duplicate", "Editing"));
            m.focusIgnored.add(Link.FOCUS_ARRANGER);
            m.loopbackCut = () -> {
                int paste = bitwig.calls.indexOf(List.of("invoke", "Paste"));
                return paste >= 0 && bitwig.calls.subList(paste, bitwig.calls.size()).contains(List.of("invoke", Link.FOCUS_LAUNCHER));
            };
        });
        Map<String, Object> request = arrange(1, "midi");
        request.put("checks", true);
        Map<String, Object> result = SelfCheckTest.map(app.waitFor(request, 30000).get("result"));
        assertEquals(2.0, result.get("launcherClipsAfter"));
        assertJson(obj("launcherPaste", obj("slot", 2, "selected", true, "focus", 1, "ran", obj("focus", false, "paste", null), "made", false,
                                            "colour", null, "sameAsMark", false, "removed", false),
                       "duplicate", obj("selected", true, "focus", 0, "ran", obj("focus", false, "duplicate", null), "launcher", 0, "tracks", 0,
                                        "removed", false)),
                   result.get("checks"));
        assertEquals(1, invoked("Paste"));
        assertEquals(0, invoked("Duplicate"));
    }

    @Test
    @DisplayName("refuses to arrange by a route it can't take, before changing anything")
    void refusesToArrangeByARouteItCantTake() {
        studio();
        assertMatches("^The route \\(via\\) has to be one of ", error(app.waitFor(arrange(1, "nope"))));
        studio(m -> m.actions.removeIf(a -> a.id().equals("Paste")));
        assertEquals("Bitwig has no \"Copy\" and \"Paste\" actions, which the link runs to copy and paste via midi.",
                     error(app.waitFor(arrange(2, "midi"))));
        // Straight away it uses the API's copy and paste, which don't need them.
        assertEquals(true, app.waitFor(arrange(3, "direct")).get("ok"));
        studio(m -> m.broken.add("getMidiOutPort"));
        assertMatches("didn't let the link set up its MIDI ports", error(app.waitFor(arrange(4, "button"))));
        assertTrue(bitwig.calls.isEmpty());
    }
}
