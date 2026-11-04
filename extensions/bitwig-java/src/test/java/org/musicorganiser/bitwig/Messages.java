package org.musicorganiser.bitwig;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.regex.Pattern;

/** The link's messages in tests: requests written as maps, and answers compared as JSON, as the JavaScript tests do. */
final class Messages {
    private Messages() {}

    /** A JSON object from names and values in turn: obj("id", 1, "cmd", "project"). */
    static Map<String, Object> obj(Object... pairs) {
        if (pairs.length % 2 != 0) throw new IllegalArgumentException("A name without a value.");
        Map<String, Object> map = new LinkedHashMap<>();
        for (int i = 0; i < pairs.length; i += 2) map.put((String) pairs[i], pairs[i + 1]);
        return map;
    }

    static List<Object> list(Object... items) {
        return new ArrayList<>(Arrays.asList(items));
    }

    /** Compares as JSON: numbers by value whatever their Java type, and objects whatever the order of their fields. */
    static void assertJson(Object expected, Object actual) {
        assertEquals(Json.parse(Json.write(expected)), Json.parse(Json.write(actual)));
    }

    /** As {@link #assertJson(Object, Object)}, saying `what` was compared when they differ. */
    static void assertJson(Object expected, Object actual, String what) {
        assertEquals(Json.parse(Json.write(expected)), Json.parse(Json.write(actual)), what);
    }

    /** What's at a path in a message: names for objects, numbers for lists. at(reply, "result", "tracks", 1, "name") */
    static Object at(Object value, Object... path) {
        Object here = value;
        for (Object step : path) {
            if (step instanceof String && here instanceof Map) here = ((Map<?, ?>) here).get(step);
            else if (step instanceof Integer && here instanceof List) here = ((List<?>) here).get((Integer) step);
            else throw new AssertionError("Nothing at " + Arrays.toString(path) + " in " + Json.write(value));
        }
        return here;
    }

    @SuppressWarnings("unchecked")
    static List<Object> listAt(Object value, Object... path) {
        return (List<Object>) at(value, path);
    }

    /** A failed reply's error message. */
    static String error(Map<String, Object> reply) {
        assertEquals(false, reply.get("ok"), () -> "expected an error, got " + Json.write(reply));
        return (String) reply.get("error");
    }

    static void assertMatches(String regex, Object text) {
        assertTrue(text instanceof String && Pattern.compile(regex).matcher((String) text).find(),
                   () -> "expected something matching /" + regex + "/, got " + text);
    }
}
