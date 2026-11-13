package org.musicorganiser.bitwig;

import com.bitwig.extension.controller.api.PlayingNote;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Comparator;
import java.util.HashMap;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * One app's watch on Bitwig (extension 3.24, the `watch` command): what of the song the link has told it last, so it
 * only sends what changed.
 *
 * While an app watches, the link looks at Bitwig every {@link #TICK} ms and sends it a `watch` event holding only the
 * parts ("sections") that changed since it last told that app: the notes playing on each track on every look, and the
 * rest (the project, the transport, the tracks, the selection, the cue markers and the level meters) every
 * {@link #SLOW_EVERY}th look. The first event after `watch` holds every part. Nothing is sent while nothing changes, so
 * a stopped Bitwig with nobody touching it sends nothing at all.
 *
 * This only reads, so it doesn't need changes allowed.
 */
final class Watch {
    /** How often the link looks at Bitwig for the apps that watch, in milliseconds: the notes playing go this often. */
    static final long TICK = 50;
    /** The other parts go every this many looks (every 100 ms), which is soon enough and keeps the feed small. */
    static final int SLOW_EVERY = 2;
    /** The section that goes on every look: the notes playing. Every other section goes with the slow looks. */
    static final String NOTES = "notes";
    /** The range the level meters are asked for in: a level is sent as a fraction of it, to 3 places. */
    static final int LEVEL_RANGE = 1000;
    /** The cue markers the link follows: the first ones in the arranger. */
    static final int MARKERS = 64;
    /** Where a section's JSON was last told, by the section's name. */
    private final Map<String, String> told = new HashMap<>();

    /** The sections of `now` this app hasn't been told as they are, which it is now taken to have been told. */
    Map<String, Object> changed(Map<String, Object> now) {
        Map<String, Object> out = new LinkedHashMap<>();
        for (Map.Entry<String, Object> e : now.entrySet()) {
            String json = Json.write(e.getValue());
            if (json.equals(told.get(e.getKey()))) continue;
            told.put(e.getKey(), json);
            out.put(e.getKey(), e.getValue());
        }
        return out;
    }

    /** A track's level: the louder of its left and right meters, as a fraction of LEVEL_RANGE, to 3 places. */
    static double level(int left, int right) {
        int loudest = Math.max(0, Math.min(LEVEL_RANGE, Math.max(left, right)));
        return Math.round(loudest * 1000.0 / LEVEL_RANGE) / 1000.0;
    }

    /** A beat position to the thousandth of a beat, so a position that hasn't moved reads the same each look. */
    static double beats(double at) {
        return Math.round(at * 1000.0) / 1000.0;
    }

    /** The notes a track is playing, as [key, velocity] pairs by key: the same notes always read the same. */
    static List<Object> notes(PlayingNote[] playing) {
        if (playing == null || playing.length == 0) return List.of();
        PlayingNote[] sorted = Arrays.copyOf(playing, playing.length);
        Arrays.sort(sorted, Comparator.comparingInt(PlayingNote::pitch).thenComparingInt(PlayingNote::velocity));
        List<Object> out = new ArrayList<>(sorted.length);
        for (PlayingNote n : sorted) out.add(List.of(n.pitch(), n.velocity()));
        return out;
    }
}
