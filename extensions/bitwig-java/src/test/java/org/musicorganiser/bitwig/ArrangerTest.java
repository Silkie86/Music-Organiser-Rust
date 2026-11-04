package org.musicorganiser.bitwig;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.musicorganiser.bitwig.FakeBitwig.CLIP_GREY;
import static org.musicorganiser.bitwig.FakeBitwig.action;
import static org.musicorganiser.bitwig.FakeBitwig.track;
import static org.musicorganiser.bitwig.Messages.assertJson;
import static org.musicorganiser.bitwig.Messages.assertMatches;
import static org.musicorganiser.bitwig.Messages.at;
import static org.musicorganiser.bitwig.Messages.error;
import static org.musicorganiser.bitwig.Messages.list;
import static org.musicorganiser.bitwig.Messages.listAt;
import static org.musicorganiser.bitwig.Messages.obj;

import com.bitwig.extension.controller.api.Application;
import java.nio.file.Path;
import java.util.Arrays;
import java.util.List;
import java.util.Map;
import java.util.function.Consumer;
import java.util.stream.Collectors;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;
import org.musicorganiser.bitwig.FakeBitwig.App;
import org.musicorganiser.bitwig.FakeBitwig.Arranged;
import org.musicorganiser.bitwig.FakeBitwig.Copied;
import org.musicorganiser.bitwig.FakeBitwig.Model;

/**
 * The arrange experiment (extension 3.6): a launcher clip copied into the arranger the way a person would do it (show the
 * Arrange layout and its clip launcher, select the clip, give the launcher the focus, copy, move the play start, bring the
 * arranger timeline to the front, paste), and the answer saying what Bitwig showed, whether it worked or not. Before it
 * copies, the link marks the launcher clip with a fresh colour, so it can tell its own clip from whatever was on Bitwig's
 * clipboard before. The stand-in Bitwig does what Bitwig is expected to, copying and pasting in whichever panel has the
 * focus, with the clip's colour; switches in its model make it ignore a step or a colour, copy from the timeline, paste
 * into the launcher instead, or ignore paste, as the real one might, and it can start with a clip on its clipboard.
 */
class ArrangerTest {
    /** Bar 5 in 4/4: four bars of four beats from the start. */
    static final double BAR_5 = 16;
    /**
     * The colours the link marks launcher clips with, one arrange after another, from the hue the stand-in starts it at
     * (0): blue, green, pink, then turquoise.
     */
    static final String MARK_1 = "#416DD9";
    static final String MARK_2 = "#9AD941";
    static final String MARK_3 = "#D941C6";
    static final String MARK_4 = "#41D9BF";
    /** MARK_1 as a Bitwig that rounds colours to its palette (Model.colourSnaps) shows it. */
    static final String MARK_1_ROUNDED = "#3366CC";
    /** The colour of Lead's clip in the arranger (see leadClipSelectedInTheArranger). */
    static final String LEAD = "#CC3333";
    /** The colour of a clip the person copied in their own song. */
    static final String THEIRS = "#2E8B57";

    @TempDir
    Path reports;

    FakeBitwig bitwig;
    App app;

    /**
     * A test project with a clip in the launcher on two tracks, Bitwig's actions that bring the arranger timeline and the
     * clip launcher to the front, and changes allowed.
     */
    void studio(Consumer<Model> extra) {
        Model m = new Model();
        m.project = "MO Live Test";
        m.selectedTrack = "Bass";
        m.tracks(
            track("Bass", "Instrument").slots("C:\\MIDI\\Bass.mid"),
            track("Lead", "Instrument").slots("C:\\MIDI\\Lead.mid", null),
            track("Master", "Master"));
        m.actions.add(action("Save", "Save", "File"));
        m.actions.add(action(Link.FOCUS_ARRANGER, "Focus/toggle Arranger Timeline Panel", "View"));
        m.actions.add(action(Link.FOCUS_LAUNCHER, "Focus/toggle Clip Launcher Panel", "View"));
        extra.accept(m);
        bitwig = FakeBitwig.start(m, reports);
        bitwig.allowChanges(true);
        app = bitwig.connect();
        app.take();
    }

    void studio() {
        studio(m -> {});
    }

    /** As {@link #studio(Consumer)}, without Bitwig's action that brings the clip launcher to the front. */
    void studioWithoutLauncherFocus(Consumer<Model> extra) {
        studio(m -> {
            m.actions.removeIf(a -> a.id().equals(Link.FOCUS_LAUNCHER));
            extra.accept(m);
        });
    }

    /** Lead's clip, in the arranger at the start and selected there, as if someone had just clicked it in the timeline. */
    void leadClipSelectedInTheArranger() {
        Arranged lead = new Arranged(bitwig.state.tracks.get(1), 0, "C:\\MIDI\\Lead.mid", LEAD);
        bitwig.state.arranged.add(lead);
        bitwig.state.arrangerSelected = lead;
        bitwig.refresh();
    }

    /**
     * Arrange, straight away (the "direct" route, with the API's copy and paste): these tests are about arrange's steps,
     * and RoutesTest about the routes, the one a request without one gets among them.
     */
    static Map<String, Object> arrange(Object id, Object track, Object slot, Object at) {
        return obj("id", id, "cmd", "arrange", "track", track, "slot", slot, "at", at, "via", "direct");
    }

    /** The clip selected in the arranger, as the answer says it: on a track, or none (null). */
    static Map<String, Object> clip(String track) {
        return obj("exists", track != null, "track", track);
    }

    static Map<String, Object> shown(boolean before, boolean after) {
        return obj("before", before, "after", after);
    }

    /**
     * How the answer says the link marked the launcher clip: its colour before, the one asked for, the one it got, whether
     * it took, and whether it's exact (the very colour asked for).
     */
    static Map<String, Object> mark(String before, String asked, String got, boolean took, boolean exact) {
        return obj("before", before, "asked", asked, "got", got, "took", took, "exact", exact);
    }

    /**
     * Whether each step ran by its route (extension 3.7): giving the launcher the focus, the copy, bringing the timeline to
     * the front and the paste (null for a step the link didn't get to).
     */
    static Map<String, Object> ran(Boolean launcherFocus, Boolean copy, Boolean focus, Boolean paste) {
        return obj("launcherFocus", launcherFocus, "copy", copy, "focus", focus, "paste", paste);
    }

    /** Bass's launcher clip pasted into the arranger at `start`, in `colour`. */
    Arranged bassAt(double start, String colour) {
        return new Arranged(bitwig.state.tracks.get(0), start, "C:\\MIDI\\Bass.mid", colour);
    }

    @Test
    @DisplayName("puts a copy of a launcher clip in the arranger at bar 5, on the clip's own track, and says so")
    void arrangesALauncherClip() {
        studio();
        Map<String, Object> reply = app.waitFor(arrange(1, 0, 0, BAR_5));
        assertJson(obj("id", 1, "ok", true, "result", obj(
                       "arranged", true, "appeared", true, "sameAsSource", true, "copiedFromTimeline", false, "at", 16, "layout", "ARRANGE",
                       "launcher", shown(true, true), "mark", mark(CLIP_GREY, MARK_1, MARK_1, true, true), "launcherFocus", 1, "timeline", shown(true, true),
                       "focus", 1, "before", clip(null), "beforePaste", clip(null), "after", clip("Bass"), "pastedColor", MARK_1,
                       "launcherClipsBefore", 1, "launcherClipsAfter", 1, "via", "direct", "ran", ran(true, true, true, true))),
                   reply);
        // The clip that appeared has the colour the launcher clip got, so it's a copy of that clip.
        assertEquals(at(reply, "result", "mark", "got"), at(reply, "result", "pastedColor"));
        assertEquals(List.of(bassAt(BAR_5, MARK_1)), bitwig.state.arranged);
        assertEquals(List.of("C:\\MIDI\\Bass.mid"), bitwig.state.tracks.get(0).slots, "the launcher keeps its clip");
        assertEquals(Map.of(0, MARK_1), bitwig.state.tracks.get(0).clipColours, "in its marking colour");
        // Select, colour the clip, give the launcher the focus, copy, move the play start, bring the timeline to the front,
        // paste: the layout was Arrange already, with its clip launcher showing, so that's left alone.
        assertEquals(List.of("selectChannel", "selectSlot", "clipColour", "invoke", "copy", "playStart", "invoke", "paste"), calls());
        assertEquals(List.of(List.of("selectSlot", 0, 0), List.of("clipColour", 0, 0, MARK_1), List.of("invoke", Link.FOCUS_LAUNCHER),
                             List.of("playStart", BAR_5), List.of("invoke", Link.FOCUS_ARRANGER)),
                     bitwig.calls.stream().filter(c -> c.size() > 1 && !c.get(0).equals("selectChannel")).collect(Collectors.toList()));

        // Without `at`, it goes where the play start is (still bar 5), and the clip selected in the arranger moves to Lead,
        // in Lead's clip's marking colour. The timeline has the focus after the paste: the launcher gets it back before the
        // copy.
        Object next = app.waitFor(obj("id", 2, "cmd", "arrange", "track", 1, "slot", 0, "via", "direct")).get("result");
        assertJson(obj("arranged", true, "appeared", true, "sameAsSource", true, "copiedFromTimeline", false, "at", 16,
                       "mark", mark(CLIP_GREY, MARK_2, MARK_2, true, true), "launcherFocus", 1, "focus", 1, "before", clip("Bass"), "after", clip("Lead"),
                       "pastedColor", MARK_2),
                   SelfCheckTest.pick(SelfCheckTest.map(next), "arranged", "appeared", "sameAsSource", "copiedFromTimeline", "at", "mark",
                                      "launcherFocus", "focus", "before", "after", "pastedColor"));
        assertEquals(List.of(BAR_5, BAR_5), bitwig.state.arranged.stream().map(Arranged::start).collect(Collectors.toList()));
        assertEquals(new Arranged(bitwig.state.tracks.get(1), BAR_5, "C:\\MIDI\\Lead.mid", MARK_2), bitwig.state.arranged.get(1));
    }

    @Test
    @DisplayName("switches to the Arrange layout, and shows its clip launcher and the arranger timeline when they're hidden")
    void switchesToTheArrangeLayout() {
        studio(m -> {
            m.layout = Application.PANEL_LAYOUT_MIX;
            m.timeline = false;
            m.launcher = false;
            m.playStart = 8;
        });
        Object result = app.waitFor(obj("id", 1, "cmd", "arrange", "track", 0, "slot", 0, "via", "direct")).get("result");
        assertJson(obj("arranged", true, "at", 8, "layout", "MIX", "launcher", shown(false, true), "launcherFocus", 1, "timeline", shown(false, true),
                       "focus", 1),
                   SelfCheckTest.pick(SelfCheckTest.map(result), "arranged", "at", "layout", "launcher", "launcherFocus", "timeline", "focus"));
        assertEquals(List.of("setPanelLayout", "showLauncher", "selectChannel", "selectSlot", "clipColour", "invoke", "copy", "playStart", "invoke",
                             "paste"),
                     calls());
        assertEquals(List.of(List.of("setPanelLayout", "ARRANGE"), List.of("showLauncher", true)), bitwig.calls.subList(0, 2));
        assertEquals(8.0, bitwig.state.arranged.get(0).start());
    }

    @Test
    @DisplayName("gives the clip launcher the focus before it copies, so a timeline in front doesn't spoil the copy")
    void givesTheLauncherTheFocusBeforeCopying() {
        studio(m -> m.focus = FakeBitwig.TIMELINE);
        leadClipSelectedInTheArranger();
        Object result = app.waitFor(arrange(1, 0, 0, BAR_5)).get("result");
        assertJson(obj("arranged", true, "sameAsSource", true, "copiedFromTimeline", false, "launcherFocus", 1, "focus", 1, "before", clip("Lead"),
                       "after", clip("Bass"), "pastedColor", MARK_1),
                   SelfCheckTest.pick(SelfCheckTest.map(result), "arranged", "sameAsSource", "copiedFromTimeline", "launcherFocus", "focus", "before",
                                      "after", "pastedColor"));
        assertEquals(new Copied("C:\\MIDI\\Bass.mid", MARK_1), bitwig.clipboard);
        assertEquals(bassAt(BAR_5, MARK_1), bitwig.state.arranged.get(1));
    }

    @Test
    @DisplayName("runs Bitwig's launcher focus action again when it hid the clip launcher, which was in front already")
    void focusesTheLauncherAgainWhenItHid() {
        // Selecting the slot gave the launcher the focus, as clicking it would.
        studio(m -> m.selectFocuses = true);
        Object result = app.waitFor(arrange(1, 0, 0, BAR_5)).get("result");
        assertJson(obj("arranged", true, "launcher", shown(true, true), "launcherFocus", 2, "focus", 1, "after", clip("Bass")),
                   SelfCheckTest.pick(SelfCheckTest.map(result), "arranged", "launcher", "launcherFocus", "focus", "after"));
        assertEquals(List.of("selectChannel", "selectSlot", "clipColour", "invoke", "invoke", "copy", "playStart", "invoke", "paste"), calls());
    }

    @Test
    @DisplayName("doesn't paste when the arranger timeline had the focus as it copied (its focus action hid the timeline), and says so")
    void doesntPasteWhatItCopiedFromTheTimeline() {
        // Without Bitwig's launcher focus action, and with Lead's clip selected in the timeline, which has the focus: Bitwig
        // copies that, not Bass's launcher clip. Bringing the timeline to the front then hides it, which is how the link knows.
        studioWithoutLauncherFocus(m -> m.focus = FakeBitwig.TIMELINE);
        leadClipSelectedInTheArranger();
        assertJson(obj("arranged", false, "appeared", false, "sameAsSource", false, "copiedFromTimeline", true, "at", 16, "layout", "ARRANGE",
                       "launcher", shown(true, true), "mark", mark(CLIP_GREY, MARK_1, MARK_1, true, true), "launcherFocus", 0, "timeline", shown(true, true),
                       "focus", 2, "before", clip("Lead"), "beforePaste", clip("Lead"), "after", clip("Lead"), "pastedColor", LEAD,
                       "launcherClipsBefore", 1, "launcherClipsAfter", 1, "via", "direct", "ran", ran(null, true, true, null)),
                   app.waitFor(arrange(1, 0, 0, BAR_5)).get("result"));
        assertEquals(new Copied("C:\\MIDI\\Lead.mid", LEAD), bitwig.clipboard, "Bitwig copied from the timeline");
        assertEquals(List.of("selectChannel", "selectSlot", "clipColour", "copy", "playStart", "invoke", "invoke"), calls());
        assertEquals(1, bitwig.state.arranged.size(), "nothing pasted");
        assertEquals(List.of("C:\\MIDI\\Bass.mid"), bitwig.state.tracks.get(0).slots);

        // The same when Bitwig has the action but ignores it.
        studio(m -> {
            m.focus = FakeBitwig.TIMELINE;
            m.focusIgnored.add(Link.FOCUS_LAUNCHER);
        });
        leadClipSelectedInTheArranger();
        assertJson(obj("arranged", false, "appeared", false, "sameAsSource", false, "copiedFromTimeline", true, "launcherFocus", 1, "focus", 2),
                   SelfCheckTest.pick(SelfCheckTest.map(app.waitFor(arrange(1, 0, 0, BAR_5)).get("result")),
                                      "arranged", "appeared", "sameAsSource", "copiedFromTimeline", "launcherFocus", "focus"));
        assertEquals(List.of("selectChannel", "selectSlot", "clipColour", "invoke", "copy", "playStart", "invoke", "invoke"), calls());
        assertEquals(1, bitwig.state.arranged.size(), "nothing pasted");
    }

    @Test
    @DisplayName("copies from the launcher without Bitwig's launcher focus action when the launcher has the focus")
    void copiesWithoutTheLauncherFocusAction() {
        studioWithoutLauncherFocus(m -> m.selectFocuses = true);
        Object result = app.waitFor(arrange(1, 0, 0, BAR_5)).get("result");
        assertJson(obj("arranged", true, "sameAsSource", true, "copiedFromTimeline", false, "launcherFocus", 0, "focus", 1, "after", clip("Bass")),
                   SelfCheckTest.pick(SelfCheckTest.map(result), "arranged", "sameAsSource", "copiedFromTimeline", "launcherFocus", "focus", "after"));
        assertEquals(List.of("selectChannel", "selectSlot", "clipColour", "copy", "playStart", "invoke", "paste"), calls());
    }

    @Test
    @DisplayName("says so when Bitwig pasted the clip into the launcher instead")
    void saysWhenBitwigPastedIntoTheLauncher() {
        studio(m -> m.pasteInLauncher = true);
        assertJson(obj("arranged", false, "appeared", false, "sameAsSource", false, "copiedFromTimeline", false, "at", 16, "layout", "ARRANGE",
                       "launcher", shown(true, true), "mark", mark(CLIP_GREY, MARK_1, MARK_1, true, true), "launcherFocus", 1, "timeline", shown(true, true),
                       "focus", 1, "before", clip(null), "beforePaste", clip(null), "after", clip(null), "pastedColor", null,
                       "launcherClipsBefore", 1, "launcherClipsAfter", 2, "via", "direct", "ran", ran(true, true, true, true)),
                   app.waitFor(arrange(1, 0, 0, BAR_5)).get("result"));
        assertEquals(List.of("C:\\MIDI\\Bass.mid", "C:\\MIDI\\Bass.mid"), bitwig.state.tracks.get(0).slots);
        assertEquals(Map.of(0, MARK_1, 1, MARK_1), bitwig.state.tracks.get(0).clipColours, "the copy keeps the clip's colour");
        assertEquals(List.of(), bitwig.state.arranged);
    }

    @Test
    @DisplayName("says so when nothing appeared, once it has waited for Bitwig, and undoes nothing")
    void saysWhenNothingAppeared() {
        studio(m -> m.pasteIgnored = true);
        app.send(arrange(1, 0, 0, BAR_5));
        bitwig.clock.advance(5000);
        assertEquals(List.of(), app.take(), "waits for Bitwig");
        bitwig.clock.advance(5000);
        Map<String, Object> reply = app.take().get(0);
        assertJson(obj("arranged", false, "appeared", false, "sameAsSource", false, "before", clip(null), "beforePaste", clip(null), "after", clip(null),
                       "pastedColor", null, "launcherClipsBefore", 1, "launcherClipsAfter", 1),
                   SelfCheckTest.pick(SelfCheckTest.map(reply.get("result")), "arranged", "appeared", "sameAsSource", "before", "beforePaste", "after",
                                      "pastedColor", "launcherClipsBefore", "launcherClipsAfter"));
        assertEquals(List.of("selectChannel", "selectSlot", "clipColour", "invoke", "copy", "playStart", "invoke", "paste"), calls());
        // The play start stays where the link put it: only test projects are sent this.
        assertEquals(16.0, bitwig.playStart.v);
    }

    @Test
    @DisplayName("stops at the step Bitwig doesn't go along with, saying which, and pastes nothing")
    void stopsAtAStepBitwigIgnores() {
        // Each step, with what the link asked of Bitwig by then: nothing is coloured or copied before the clip is selected
        // (with the launcher showing, and in front), and nothing is pasted unless every step went through.
        fails(m -> {
            m.layout = Application.PANEL_LAYOUT_MIX;
            m.layoutIgnored = true;
        }, "Bitwig didn't switch to its Arrange layout.", "setPanelLayout");
        fails(m -> {
            m.launcher = false;
            m.launcherIgnored = true;
        }, "Bitwig didn't show the clip launcher in its Arrange layout.", "showLauncher");
        fails(m -> m.selectIgnored = true, "Bitwig didn't select the clip in slot 0 of track 0.", "selectChannel", "selectSlot");
        fails(m -> {
            m.selectFocuses = true;
            m.focusOnlyHides.add(Link.FOCUS_LAUNCHER);
        }, "Bitwig hid its clip launcher, which was in front already, and didn't show it again.", "selectChannel", "selectSlot", "clipColour", "invoke",
              "invoke");
        fails(m -> m.playStartIgnored = true, "Bitwig didn't move the play start to beat 16.", "selectChannel", "selectSlot", "clipColour", "invoke", "copy",
              "playStart");
        fails(m -> {
            m.timeline = false;
            m.focusIgnored.add(Link.FOCUS_ARRANGER);
        }, "Bitwig didn't show its arranger timeline.", "selectChannel", "selectSlot", "clipColour", "invoke", "copy", "playStart", "invoke");
        fails(m -> {
            m.focus = FakeBitwig.TIMELINE;
            m.focusIgnored.add(Link.FOCUS_LAUNCHER);
            m.focusOnlyHides.add(Link.FOCUS_ARRANGER);
        }, "Bitwig hid its arranger timeline, which was in front already, and didn't show it again.",
              "selectChannel", "selectSlot", "clipColour", "invoke", "copy", "playStart", "invoke", "invoke");
    }

    /** Runs arrange in a studio set up by `setup`, which fails with `error` once the link has asked Bitwig for `calls`, and no more. */
    private void fails(Consumer<Model> setup, String error, String... calls) {
        studio(setup);
        assertJson(obj("id", 1, "ok", false, "error", error), app.waitFor(arrange(1, 0, 0, BAR_5)));
        assertEquals(List.of(calls), calls(), error);
        bitwig.clock.advance(10000);
        assertEquals(List.of(calls), calls(), "and nothing more afterwards: " + error);
        assertEquals(List.of(), bitwig.state.arranged, error);
        assertEquals(List.of("C:\\MIDI\\Bass.mid"), bitwig.state.tracks.get(0).slots, error);
    }

    @Test
    @DisplayName("isn't fooled by a clip that was on Bitwig's clipboard already when the copy didn't take")
    void tellsItsOwnClipFromWhatWasOnTheClipboard() {
        // The reviewers' case: the person copied a clip in their own song, Bitwig ignores the launcher focus action and
        // another panel has the focus, so copy does nothing. Paste puts down their clip, which appears on the track all the
        // same, but in its own colour, not the launcher clip's.
        studio(m -> {
            m.clipboard = new Copied("D:\\Songs\\Night Drive\\Hook.mid", THEIRS);
            m.focus = "";
            m.focusIgnored.add(Link.FOCUS_LAUNCHER);
        });
        Object result = app.waitFor(arrange(1, 0, 0, BAR_5)).get("result");
        assertJson(obj("arranged", false, "appeared", true, "sameAsSource", false, "copiedFromTimeline", false,
                       "mark", mark(CLIP_GREY, MARK_1, MARK_1, true, true), "launcherFocus", 1, "focus", 1, "beforePaste", clip(null),
                       "after", clip("Bass"), "pastedColor", THEIRS, "launcherClipsBefore", 1, "launcherClipsAfter", 1),
                   SelfCheckTest.pick(SelfCheckTest.map(result), "arranged", "appeared", "sameAsSource", "copiedFromTimeline", "mark", "launcherFocus",
                                      "focus", "beforePaste", "after", "pastedColor", "launcherClipsBefore", "launcherClipsAfter"));
        assertEquals(new Copied("D:\\Songs\\Night Drive\\Hook.mid", THEIRS), bitwig.clipboard, "the copy didn't take");
        assertEquals(List.of(new Arranged(bitwig.state.tracks.get(0), BAR_5, "D:\\Songs\\Night Drive\\Hook.mid", THEIRS)), bitwig.state.arranged);
        assertEquals(List.of("selectChannel", "selectSlot", "clipColour", "invoke", "copy", "playStart", "invoke", "paste"), calls());
    }

    @Test
    @DisplayName("carries on when Bitwig won't colour the launcher clip, but never says it arranged the clip then")
    void cantTellItsClipWhenTheColourDoesntTake() {
        studio(m -> m.colourIgnored = true);
        app.send(arrange(1, 0, 0, BAR_5));
        bitwig.clock.advance(4000);
        assertFalse(calls().contains("copy"), "waits for Bitwig to colour the clip");
        bitwig.clock.advance(6000);
        Object result = app.take().get(0).get("result");
        // The clip went in, but in the colour it always had, which a clip on the clipboard before could have had too.
        assertJson(obj("arranged", false, "appeared", true, "sameAsSource", false, "mark", mark(CLIP_GREY, MARK_1, CLIP_GREY, false, false),
                       "after", clip("Bass"), "pastedColor", CLIP_GREY),
                   SelfCheckTest.pick(SelfCheckTest.map(result), "arranged", "appeared", "sameAsSource", "mark", "after", "pastedColor"));
        assertEquals(List.of(bassAt(BAR_5, CLIP_GREY)), bitwig.state.arranged);
        assertEquals(List.of("selectChannel", "selectSlot", "clipColour", "invoke", "copy", "playStart", "invoke", "paste"), calls());
    }

    @Test
    @DisplayName("compares with the colour Bitwig rounds the mark to, but never says it arranged the clip then")
    void comparesWithTheColourTheClipGot() {
        studio(m -> m.colourSnaps = true);
        Object result = app.waitFor(arrange(1, 0, 0, BAR_5)).get("result");
        // The launcher clip's copy went in, in the colour the clip got. But that's one of Bitwig's own, which another clip
        // (one already on Bitwig's clipboard, say) can have too, so the link can't be sure it's the launcher clip's copy.
        assertJson(obj("arranged", false, "appeared", true, "sameAsSource", true, "mark", mark(CLIP_GREY, MARK_1, MARK_1_ROUNDED, true, false),
                       "pastedColor", MARK_1_ROUNDED),
                   SelfCheckTest.pick(SelfCheckTest.map(result), "arranged", "appeared", "sameAsSource", "mark", "pastedColor"));
        assertEquals(List.of(bassAt(BAR_5, MARK_1_ROUNDED)), bitwig.state.arranged);
    }

    @Test
    @DisplayName("isn't fooled by a clip on Bitwig's clipboard in the colour Bitwig rounded the mark to")
    void isntFooledByAClipInTheRoundedColour() {
        // The copy does nothing (another panel has the focus, and Bitwig ignores the launcher focus action), and what's on
        // Bitwig's clipboard is a clip in one of Bitwig's own colours: one the person copied, or an earlier run's copy of a
        // clip whose mark Bitwig rounded the same way. Bitwig rounds this run's mark to that colour as well, so the clip
        // paste puts down has the colour the launcher clip got, though it isn't the launcher clip's copy.
        studio(m -> {
            m.colourSnaps = true;
            m.clipboard = new Copied("D:\\Songs\\Night Drive\\Hook.mid", MARK_1_ROUNDED);
            m.focus = "";
            m.focusIgnored.add(Link.FOCUS_LAUNCHER);
        });
        Object result = app.waitFor(arrange(1, 0, 0, BAR_5)).get("result");
        assertJson(obj("arranged", false, "appeared", true, "sameAsSource", true, "copiedFromTimeline", false,
                       "mark", mark(CLIP_GREY, MARK_1, MARK_1_ROUNDED, true, false), "after", clip("Bass"), "pastedColor", MARK_1_ROUNDED),
                   SelfCheckTest.pick(SelfCheckTest.map(result), "arranged", "appeared", "sameAsSource", "copiedFromTimeline", "mark", "after",
                                      "pastedColor"));
        assertEquals(new Copied("D:\\Songs\\Night Drive\\Hook.mid", MARK_1_ROUNDED), bitwig.clipboard, "the copy didn't take");
        assertEquals(List.of(new Arranged(bitwig.state.tracks.get(0), BAR_5, "D:\\Songs\\Night Drive\\Hook.mid", MARK_1_ROUNDED)),
                     bitwig.state.arranged);
    }

    @Test
    @DisplayName("marks each arrange with a colour of its own, unlike the clip's and the last mark")
    void marksEachArrangeInAFreshColour() {
        studio();
        Object first = app.waitFor(arrange(1, 0, 0, BAR_5)).get("result");
        assertJson(mark(CLIP_GREY, MARK_1, MARK_1, true, true), at(first, "mark"));
        // The same clip again, with nothing selected in the arranger now (as if someone had clicked an empty spot there): it
        // has the first mark's colour, and gets another.
        bitwig.state.arrangerSelected = null;
        bitwig.refresh();
        Object second = app.waitFor(arrange(2, 0, 0, 32)).get("result");
        assertJson(obj("arranged", true, "sameAsSource", true, "mark", mark(MARK_1, MARK_2, MARK_2, true, true), "pastedColor", MARK_2),
                   SelfCheckTest.pick(SelfCheckTest.map(second), "arranged", "sameAsSource", "mark", "pastedColor"));
        assertEquals(List.of(bassAt(BAR_5, MARK_1), bassAt(32, MARK_2)), bitwig.state.arranged);

        // A clip that already has the colour that would come next gets the one after.
        bitwig.state.tracks.get(1).clipColours.put(0, MARK_3);
        bitwig.refresh();
        Object third = app.waitFor(arrange(3, 1, 0, BAR_5)).get("result");
        assertJson(obj("arranged", true, "mark", mark(MARK_3, MARK_4, MARK_4, true, true), "pastedColor", MARK_4),
                   SelfCheckTest.pick(SelfCheckTest.map(third), "arranged", "mark", "pastedColor"));
        assertEquals(List.of(List.of("clipColour", 0, 0, MARK_1), List.of("clipColour", 0, 0, MARK_2), List.of("clipColour", 1, 0, MARK_4)),
                     bitwig.calls.stream().filter(c -> c.get(0).equals("clipColour")).collect(Collectors.toList()));
    }

    @Test
    @DisplayName("refuses a slot without a clip, one out of reach and an odd place, before touching anything")
    void refusesWhatItCantArrange() {
        studio();
        assertEquals("Slot 1 on that track has no clip to put in the arranger.", error(app.ask(arrange(1, 1, 1, BAR_5))));
        assertEquals("Slot 4 on that track has no clip to put in the arranger.", error(app.ask(arrange(2, 1, 4, BAR_5))));
        assertEquals("The app can reach the first 8 clip slots on a track, not slot 8.", error(app.ask(arrange(3, 0, 8, BAR_5))));
        assertMatches("not slot undefined", error(app.ask(obj("id", 4, "cmd", "arrange", "track", 0))));
        assertMatches("no track 9", error(app.ask(arrange(5, 9, 0, BAR_5))));
        for (Object at : list(-1, "16", true, list(16), obj()))
            assertMatches("^Say where in the arranger as beats from the start \\(0 or more\\), not ", error(app.ask(arrange(6, 0, 0, at))));
        assertEquals(List.of(), bitwig.calls);
        assertEquals(List.of(), bitwig.state.arranged);
    }

    @Test
    @DisplayName("needs changes turned on, and keeps to the project the request names")
    void needsChangesTurnedOn() {
        studio();
        bitwig.allowChanges(false);
        assertMatches(LinkTest.CHANGES_OFF, error(app.ask(arrange(1, 0, 0, BAR_5))));
        bitwig.allowChanges(true);
        assertEquals(LinkTest.elsewhere("MO Live Test", "Night Drive"),
                     error(app.ask(obj("id", 2, "cmd", "arrange", "track", 0, "slot", 0, "onlyIn", "Night Drive", "via", "direct"))));
        assertEquals(List.of(), bitwig.calls);

        // Another project comes to the front once the clip is being selected: nothing is copied or pasted.
        bitwig.switchDuring("arrange", () -> calls().contains("selectSlot"), "My Song");
        assertJson(obj("id", 3, "ok", false, "error", LinkTest.elsewhere("My Song", "MO Live Test")),
                   app.waitFor(obj("id", 3, "cmd", "arrange", "track", 0, "slot", 0, "onlyIn", "MO Live Test", "via", "direct")));
        assertEquals(List.of("selectChannel", "selectSlot"), calls());
        assertEquals(List.of(), bitwig.state.arranged);
    }

    @Test
    @DisplayName("says when Bitwig has no action to bring the arranger timeline to the front")
    void needsBitwigsFocusAction() {
        studio(m -> m.actions.removeIf(a -> a.id().equals(Link.FOCUS_ARRANGER)));
        assertEquals("Bitwig has no action \"focus_or_toggle_arranger\", which the link runs to bring the arranger timeline to the front.",
                     error(app.ask(arrange(1, 0, 0, BAR_5))));
        assertEquals(List.of(), bitwig.calls);
    }

    @Test
    @DisplayName("still starts in a Bitwig without an arranger for extensions, and says the self-check tells why")
    void startsWithoutTheArranger() {
        studio(m -> m.broken.add("createArranger"));
        assertEquals("MO Live Test", at(app.ask(obj("id", 1, "cmd", "hello")), "result", "project"));
        assertEquals(true, app.ask(obj("id", 2, "cmd", "project")).get("ok"));
        assertEquals("This Bitwig didn't let the link reach its arranger. A selftest says why.", error(app.ask(arrange(3, 0, 0, BAR_5))));

        Map<String, Object> result = SelfCheckTest.map(app.ask(obj("id", 4, "cmd", "selftest")).get("result"));
        assertEquals(SelfCheckTest.CHECKS, SelfCheckTest.names(result));
        assertEquals(List.of("arranger"), listAt(result, "checks").stream().filter(c -> !(Boolean) at(c, "ok")).map(c -> at(c, "name"))
                                                                  .collect(Collectors.toList()));
        assertMatches("^Couldn't set it up: java.lang.AbstractMethodError: ", SelfCheckTest.detail(result, "arranger"));
        assertMatches("couldn't set up the arranger", bitwig.errors.get(0));
        assertEquals(List.of(), bitwig.calls);
    }

    // ---- Checks (extension 3.8) ----

    /** Arrange with its checks asked for, straight away (as {@link #arrange}). */
    static Map<String, Object> arrangeChecked(Object id) {
        Map<String, Object> request = arrange(id, 0, 0, BAR_5);
        request.put("checks", true);
        return request;
    }

    /** How a check says it selected its slot and gave the launcher the focus, and whether each step ran. */
    static Map<String, Object> launcherPaste(int slot, int focus, boolean made, String colour, boolean same, boolean removed) {
        return obj("slot", slot, "selected", true, "focus", focus, "ran", obj("focus", true, "paste", true), "made", made, "colour", colour,
                   "sameAsMark", same, "removed", removed);
    }

    static Map<String, Object> duplicate(int focus, int launcher, int tracks, boolean removed) {
        return obj("selected", true, "focus", focus, "ran", obj("focus", true, "duplicate", true), "launcher", launcher, "tracks", tracks,
                   "removed", removed);
    }

    /** As {@link #studio(Consumer)}, with Bitwig's Duplicate action too. */
    void studioWithDuplicate(Consumer<Model> extra) {
        studio(m -> {
            m.actions.add(action("Duplicate", "Duplicate", "Editing"));
            extra.accept(m);
        });
    }

    @Test
    @DisplayName("runs no checks when the clip landed in the arranger, or when they weren't asked for")
    void checksOnlyWhenAskedAndItFailed() {
        studioWithDuplicate(m -> {});
        Map<String, Object> result = SelfCheckTest.map(app.waitFor(arrangeChecked(1)).get("result"));
        assertEquals(true, result.get("arranged"));
        assertFalse(result.containsKey("checks"));

        studioWithDuplicate(m -> m.pasteIgnored = true);
        result = SelfCheckTest.map(app.waitFor(arrange(1, 0, 0, BAR_5)).get("result"));
        assertEquals(false, result.get("arranged"));
        assertFalse(result.containsKey("checks"));
    }

    @Test
    @DisplayName("checks a paste and a Duplicate in the launcher when the clip didn't land in the arranger, and tidies up after them")
    void checksInTheLauncher() {
        // Bitwig ignores the action that brings the arranger timeline to the front, so the paste goes into the launcher.
        studioWithDuplicate(m -> m.focusIgnored.add(Link.FOCUS_ARRANGER));
        Map<String, Object> result = SelfCheckTest.map(app.waitFor(arrangeChecked(1)).get("result"));
        assertJson(obj("arranged", false, "launcherClipsBefore", 1, "launcherClipsAfter", 2),
                   SelfCheckTest.pick(result, "arranged", "launcherClipsBefore", "launcherClipsAfter"));
        // The paste into an empty slot (2: arrange's own paste went into 1) made the clip in its marking colour, so the copy
        // took; Duplicate made another launcher clip, so editing works in the launcher. The launcher was still in front each
        // time (the timeline never got the focus), so its focus action hid it and ran again. Both clips the checks made
        // were deleted.
        assertJson(obj("launcherPaste", launcherPaste(2, 2, true, MARK_1, true, true), "duplicate", duplicate(2, 1, 0, true)), result.get("checks"));
        assertEquals(Arrays.asList("C:\\MIDI\\Bass.mid", "C:\\MIDI\\Bass.mid", null), bitwig.state.tracks.get(0).slots.subList(0, 3));
        assertEquals(List.of("selectChannel", "selectSlot", "invoke", "invoke", "paste", "deleteClip", "selectChannel", "selectSlot", "invoke",
                             "invoke", "invoke", "deleteClip"),
                     calls().subList(calls().lastIndexOf("playStart") + 3, calls().size()));
        assertEquals(List.of("deleteClip", 0, 2), bitwig.calls.get(bitwig.calls.size() - 1));
    }

    @Test
    @DisplayName("says so in its checks when the copy didn't take")
    void checksWhenTheCopyDidntTake() {
        // Bitwig ignores copy, so its clipboard still holds the clip someone copied in their own song, and the timeline's
        // focus action, so arrange's paste puts that clip in the launcher.
        studioWithDuplicate(m -> {
            m.copyIgnored = true;
            m.clipboard = new Copied("D:\\Songs\\Night Drive\\Hook.mid", THEIRS);
            m.focusIgnored.add(Link.FOCUS_ARRANGER);
        });
        Map<String, Object> result = SelfCheckTest.map(app.waitFor(arrangeChecked(1)).get("result"));
        // The check's paste into the empty slot made a clip in their colour, not the mark's (deleted again), while
        // Duplicate worked in the launcher.
        assertJson(obj("launcherPaste", launcherPaste(2, 2, true, THEIRS, false, true), "duplicate", duplicate(2, 1, 0, true)), result.get("checks"));
        assertEquals(Arrays.asList("C:\\MIDI\\Bass.mid", "D:\\Songs\\Night Drive\\Hook.mid", null), bitwig.state.tracks.get(0).slots.subList(0, 3));
        assertEquals(List.of(), bitwig.state.arranged);
    }

    @Test
    @DisplayName("runs no checks when arrange didn't paste, when its paste put something in the arranger, or without the launcher's focus action")
    void checksOnlyWhereTheyCantLeaveAnythingBehind() {
        // The timeline had the focus as the link copied, so arrange didn't paste: a check's paste would put down what the
        // timeline's copy left on the clipboard.
        studioWithDuplicate(m -> {
            m.focus = FakeBitwig.TIMELINE;
            m.focusIgnored.add(Link.FOCUS_LAUNCHER);
        });
        leadClipSelectedInTheArranger();
        Map<String, Object> result = SelfCheckTest.map(app.waitFor(arrangeChecked(1)).get("result"));
        assertEquals(true, result.get("copiedFromTimeline"));
        assertFalse(result.containsKey("checks"));
        assertEquals(1, bitwig.state.arranged.size(), "only Lead's clip");
        assertFalse(bitwig.calls.contains(List.of("invoke", "Duplicate")));

        // Arrange's paste put the clip someone copied in their own song in the arranger: another paste would land where
        // that one did.
        studioWithDuplicate(m -> {
            m.clipboard = new Copied("D:\\Songs\\Night Drive\\Hook.mid", THEIRS);
            m.focus = "";
            m.focusIgnored.add(Link.FOCUS_LAUNCHER);
        });
        result = SelfCheckTest.map(app.waitFor(arrangeChecked(1)).get("result"));
        assertJson(obj("arranged", false, "appeared", true, "sameAsSource", false), SelfCheckTest.pick(result, "arranged", "appeared", "sameAsSource"));
        assertFalse(result.containsKey("checks"));
        assertEquals(1, bitwig.state.arranged.size());

        // The clip landed, but Bitwig rounded its mark, so arrange can't say it's the launcher clip's copy.
        studioWithDuplicate(m -> m.colourSnaps = true);
        result = SelfCheckTest.map(app.waitFor(arrangeChecked(1)).get("result"));
        assertJson(obj("arranged", false, "appeared", true, "sameAsSource", true), SelfCheckTest.pick(result, "arranged", "appeared", "sameAsSource"));
        assertFalse(result.containsKey("checks"));

        // Bitwig put the paste on another track, so the clip selected in the arranger moved there: a check's paste could
        // land there too.
        studioWithDuplicate(m -> m.arrangerPasteOn = "Lead");
        result = SelfCheckTest.map(app.waitFor(arrangeChecked(1)).get("result"));
        assertJson(obj("appeared", false, "beforePaste", obj("exists", false, "track", null), "after", obj("exists", true, "track", "Lead")),
                   SelfCheckTest.pick(result, "appeared", "beforePaste", "after"));
        assertFalse(result.containsKey("checks"));
        assertFalse(bitwig.calls.contains(List.of("invoke", "Duplicate")));

        // A clip on the track was selected in the arranger already, and the clip the paste put there looks just like it.
        studioWithDuplicate(m -> {});
        Arranged bass = new Arranged(bitwig.state.tracks.get(0), 0, "C:\\MIDI\\Bass.mid", LEAD);
        bitwig.state.arranged.add(bass);
        bitwig.state.arrangerSelected = bass;
        bitwig.refresh();
        result = SelfCheckTest.map(app.waitFor(arrangeChecked(1)).get("result"));
        assertJson(obj("appeared", false, "beforePaste", obj("exists", true, "track", "Bass"), "after", obj("exists", true, "track", "Bass")),
                   SelfCheckTest.pick(result, "appeared", "beforePaste", "after"));
        assertFalse(result.containsKey("checks"));
        assertEquals(2, bitwig.state.arranged.size(), "Bass's clip and the one the paste put there");
        assertFalse(bitwig.calls.contains(List.of("invoke", "Duplicate")));

        // Without the launcher's focus action, nothing keeps a check's paste or Duplicate in the launcher.
        studioWithoutLauncherFocus(m -> {
            m.actions.add(action("Duplicate", "Duplicate", "Editing"));
            m.focusIgnored.add(Link.FOCUS_ARRANGER);
            m.selectFocuses = true;
        });
        result = SelfCheckTest.map(app.waitFor(arrangeChecked(1)).get("result"));
        assertEquals(2.0, result.get("launcherClipsAfter"), "arrange pasted into the launcher");
        assertFalse(result.containsKey("checks"));
        assertFalse(bitwig.calls.contains(List.of("invoke", "Duplicate")));
    }

    @Test
    @DisplayName("says so in its checks when Duplicate went to the track, as no panel had the focus")
    void checksWhenDuplicateWentToTheTrack() {
        studioWithDuplicate(m -> {
            m.focusIgnored.add(Link.FOCUS_LAUNCHER);
            m.focusIgnored.add(Link.FOCUS_ARRANGER);
        });
        // Nothing is pasted, by arrange or its check, so each waits for Bitwig.
        Map<String, Object> result = SelfCheckTest.map(app.waitFor(arrangeChecked(1), 30000).get("result"));
        assertJson(obj("launcherPaste", launcherPaste(1, 1, false, null, false, false), "duplicate", duplicate(1, 0, 1, false)), result.get("checks"));
        assertEquals(List.of("Bass", "Bass 2", "Lead", "Master"), bitwig.state.tracks.stream().map(t -> t.name).collect(Collectors.toList()));
    }

    @Test
    @DisplayName("leaves out a check it can't run: no empty slot within reach, or no Duplicate in Bitwig")
    void leavesOutChecksItCantRun() {
        studio(m -> {
            m.scenes = 2;
            m.focusIgnored.add(Link.FOCUS_ARRANGER);
        });
        // Arrange's own paste went into the launcher's second slot, the last there is.
        Map<String, Object> result = SelfCheckTest.map(app.waitFor(arrangeChecked(1)).get("result"));
        assertJson(obj("launcherPaste", null, "duplicate", null), result.get("checks"));
    }

    @Test
    @DisplayName("still answers when a check can't go on, saying why")
    void answersWhenACheckStops() {
        studioWithDuplicate(m -> m.focusIgnored.add(Link.FOCUS_ARRANGER));
        bitwig.switchDuring("arrange", () -> bitwig.calls.contains(List.of("invoke", "Duplicate")), "My Song");
        Map<String, Object> request = arrangeChecked(1);
        request.put("onlyIn", "MO Live Test");
        Map<String, Object> reply = app.waitFor(request);
        assertEquals(true, reply.get("ok"));
        assertEquals(LinkTest.elsewhere("My Song", "MO Live Test"), at(reply, "result", "checks", "error"));
        assertEquals(true, at(reply, "result", "checks", "duplicate", "ran", "duplicate"));
    }

    @Test
    @DisplayName("refuses checks that aren't true or false")
    void refusesOddChecks() {
        studio();
        Map<String, Object> request = arrange(1, 0, 0, BAR_5);
        request.put("checks", "yes");
        assertEquals("checks is true or false, not yes.", error(app.ask(request)));
        assertEquals(List.of(), bitwig.calls);
    }

    // ---- Files as tracks (extension 3.8) ----

    static Map<String, Object> fileTrack(Object id, Object track, Object path, Object at) {
        return obj("id", id, "cmd", "filetrack", "track", track, "path", path, "at", at);
    }

    static final String LIVE_CLIP = "C:\\Tests\\Live Clip.mid";

    @Test
    @DisplayName("puts a MIDI file on a new track after the one given, with the play start moved first, and says where it went")
    void putsAFileOnANewTrack() {
        studio();
        Map<String, Object> reply = app.waitFor(fileTrack(1, 0, LIVE_CLIP, BAR_5));
        assertJson(obj("id", 1, "ok", true, "result", obj(
                       "added", 1, "index", 1, "id", null, "name", "Live Clip", "type", "Instrument", "tracksBefore", 3, "tracksAfter", 4, "launcherClips", 0,
                       "selected", false, "arranger", clip("Live Clip"), "arrangerBefore", clip(null), "at", 16, "onTrack", obj("before", 1, "after", 1),
                       "newTracks", list(obj("index", 1, "id", null, "name", "Live Clip", "type", "Instrument", "group", false, "launcherClips", 0)))),
                   reply);
        assertEquals(List.of(new Arranged(bitwig.state.tracks.get(1), BAR_5, LIVE_CLIP, CLIP_GREY)), bitwig.state.arranged);
        assertEquals(List.of(List.of("playStart", BAR_5), List.of("insertFile", LIVE_CLIP, "after track 0")), bitwig.calls);
    }

    @Test
    @DisplayName("says so when Bitwig put the file in the new track's launcher, or did nothing with it, and leaves the play start alone without `at`")
    void saysWhereElseTheFileWent() {
        studio(m -> {
            m.fileGoes = "launcher";
            m.playStart = 8;
        });
        Object result = app.waitFor(fileTrack(1, 1, LIVE_CLIP, null)).get("result");
        assertJson(obj("added", 1, "index", 2, "name", "Live Clip", "launcherClips", 1, "arranger", clip(null), "at", 8, "onTrack", obj("before", 1, "after", 1)),
                   SelfCheckTest.pick(SelfCheckTest.map(result), "added", "index", "name", "launcherClips", "arranger", "at", "onTrack"));
        assertEquals(List.of(List.of("insertFile", LIVE_CLIP, "after track 1")), bitwig.calls);

        studio(m -> m.fileGoes = "nothing");
        app.send(fileTrack(1, 0, LIVE_CLIP, null));
        bitwig.clock.advance(5000);
        assertEquals(List.of(), app.take(), "waits for Bitwig");
        bitwig.clock.advance(5000);
        assertJson(obj("added", 0, "index", null, "id", null, "name", null, "type", null, "tracksBefore", 3, "tracksAfter", 3, "launcherClips", null,
                       "selected", false, "arranger", clip(null), "arrangerBefore", clip(null), "at", 0, "onTrack", obj("before", 1, "after", 1),
                       "newTracks", list()),
                   app.take().get(0).get("result"));
    }

    @Test
    @DisplayName("refuses an odd track, file or place, needs changes turned on, and keeps to the project the request names")
    void refusesOddFileTracks() {
        studio();
        assertMatches("no track 9", error(app.ask(fileTrack(1, 9, LIVE_CLIP, null))));
        assertMatches("file", error(app.ask(fileTrack(2, 0, null, null))));
        assertEquals("Say where the play start goes as beats from the start (0 or more), not -1.", error(app.ask(fileTrack(3, 0, LIVE_CLIP, -1.0))));
        bitwig.allowChanges(false);
        assertMatches(LinkTest.CHANGES_OFF, error(app.ask(fileTrack(4, 0, LIVE_CLIP, null))));
        bitwig.allowChanges(true);
        Map<String, Object> elsewhere = fileTrack(5, 0, LIVE_CLIP, null);
        elsewhere.put("onlyIn", "Night Drive");
        assertEquals(LinkTest.elsewhere("MO Live Test", "Night Drive"), error(app.ask(elsewhere)));
        assertEquals(List.of(), bitwig.calls);
    }

    /** What the extension asked of Bitwig, by name. */
    private List<Object> calls() {
        return bitwig.calls.stream().map(c -> c.get(0)).collect(Collectors.toList());
    }
}
