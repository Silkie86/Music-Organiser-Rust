package org.musicorganiser.bitwig;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import org.junit.jupiter.api.Test;

/** The JSON reader and writer, which have to agree with JavaScript's (the app was written against the script's). */
class JsonTest {
    @Test
    void readsWhatTheAppSends() {
        Object request = Json.parse("{\"id\": 7, \"cmd\": \"pads\", \"track\": null, \"pads\": [{\"note\": 36, \"path\": \"C:\\\\Kit\\\\Kick.wav\"}]}");
        Map<?, ?> map = (Map<?, ?>) request;
        assertEquals(List.of("id", "cmd", "track", "pads"), new ArrayList<>(map.keySet()), "keeps the order");
        assertEquals(7.0, map.get("id"));
        assertEquals("pads", map.get("cmd"));
        assertTrue(map.containsKey("track"));
        assertNull(map.get("track"));
        Map<?, ?> pad = (Map<?, ?>) ((List<?>) map.get("pads")).get(0);
        assertEquals(36.0, pad.get("note"));
        assertEquals("C:\\Kit\\Kick.wav", pad.get("path"));
    }

    @Test
    void readsEveryKindOfValue() {
        assertEquals(Arrays.asList(true, false, null, -0.5, 1e21, 0.0, "x"),
                     Json.parse(" [ true , false , null , -0.5 , 1E+21 , 0 , \"x\" ] "));
        assertEquals(Map.of(), Json.parse("{}"));
        assertEquals(List.of(), Json.parse("[]"));
        assertEquals("hello", Json.parse("\"hello\""));
        assertNull(Json.parse("null"));
        assertEquals(5.0, Json.parse("5"));
    }

    @Test
    void readsEscapesAndTextOutsideAscii() {
        assertEquals("a\"b\\c/d\b\f\n\r\t\u00e9\ud834\udd1e", Json.parse("\"a\\\"b\\\\c\\/d\\b\\f\\n\\r\\t\\u00E9\\ud834\\udd1e\""));
        assertEquals("Café ☕ 𝄞", Json.parse("\"Café ☕ 𝄞\""));
    }

    @Test
    void lastOfTwoSameNamesWins() {
        assertEquals(Map.of("a", 2.0), Json.parse("{\"a\": 1, \"a\": 2}"));
    }

    @Test
    void refusesWhatJavaScriptRefuses() {
        for (String bad : new String[] {
            "", "{not json", "{\"a\": 1,}", "[1,]", "[1 2]", "{'a': 1}", "{\"a\" 1}", "01", "1.", ".5", "-", "+1", "1e", "NaN",
            "Infinity", "tru", "nul", "\"\\x\"", "\"\\u12\"", "\"tab\there\"", "\"open", "[", "{\"a\":", "1 2", "// no"
        }) {
            assertThrows(Json.JsonException.class, () -> Json.parse(bad), bad);
        }
    }

    @Test
    void refusesJsonNestedTooDeeply() {
        String deep = "[".repeat(10000) + "]".repeat(10000);
        assertThrows(Json.JsonException.class, () -> Json.parse(deep));
        String fine = "[".repeat(200) + "]".repeat(200);
        Json.parse(fine);
    }

    @Test
    void writesAsJavaScriptDoes() {
        Map<String, Object> m = new LinkedHashMap<>();
        m.put("id", 1.0);
        m.put("ok", true);
        m.put("result", Map.of());
        m.put("list", Arrays.asList(1, 2L, null, "x", false));
        m.put("left out", Json.UNDEFINED);
        assertEquals("{\"id\":1,\"ok\":true,\"result\":{},\"list\":[1,2,null,\"x\",false]}", Json.write(m));
        assertEquals("[\"a\",null]", Json.write(new String[] {"a", null}));
    }

    @Test
    void writesNumbersAsJavaScriptDoes() {
        assertEquals("0", Json.write(0.0));
        assertEquals("0", Json.write(-0.0));
        assertEquals("124", Json.write(124.0));
        assertEquals("-3.5", Json.write(-3.5));
        assertEquals("0.6", Json.write(0.6));
        assertEquals("0.1", Json.write(0.1f), "a float as its own shortest digits");
        assertEquals("0.30000000000000004", Json.write(0.1 + 0.2));
        assertEquals("100", Json.write(100.0));
        assertEquals("123456789012345680000", Json.write(1.2345678901234568e20));
        assertEquals("1e+21", Json.write(1e21));
        assertEquals("1.5e+300", Json.write(1.5e300));
        assertEquals("0.000001", Json.write(1e-6));
        assertEquals("1e-7", Json.write(1e-7));
        assertEquals("1.25e-7", Json.write(1.25e-7));
        assertEquals("1.7976931348623157e+308", Json.write(Double.MAX_VALUE));
        assertEquals("9007199254740993", Json.write(9007199254740993L), "a long as it is");
        assertEquals("null", Json.write(Double.NaN), "NaN (an unreadable parameter) is null");
        assertEquals("null", Json.write(Double.POSITIVE_INFINITY));
    }

    @Test
    void writesPlainAsciiWithEscapes() {
        assertEquals("\"Caf\\u00e9 \\u2615 \\ud834\\udd1e \\u007f\"", Json.write("Café ☕ 𝄞 \u007f"));
        assertEquals("\"\\\"\\\\\\b\\f\\n\\r\\t\\u0001\\u001f/\"", Json.write("\"\\\b\f\n\r\t\u0001\u001f/"));
        String text = "Ünïcödé \ud800 lone";
        assertEquals(text, Json.parse(Json.write(text)), "reads back the same");
    }

    @Test
    void writesIndentedForPeopleToRead() {
        Map<String, Object> m = new LinkedHashMap<>();
        m.put("ok", true);
        m.put("checks", List.of(Map.of("name", "x"), List.of()));
        m.put("empty", Map.of());
        assertEquals("{\n  \"ok\": true,\n  \"checks\": [\n    {\n      \"name\": \"x\"\n    },\n    []\n  ],\n  \"empty\": {}\n}",
                     Json.pretty(m));
        assertEquals(m, Json.parse(Json.pretty(m)), "reads back the same");
    }

    @Test
    void refusesToWriteWhatIsntJson() {
        assertThrows(IllegalArgumentException.class, () -> Json.write(new Object()));
    }

    @Test
    void showsValuesAsJavaScriptsStringDoes() {
        assertEquals("undefined", Json.js(Json.UNDEFINED));
        assertEquals("null", Json.js(null));
        assertEquals("3", Json.js(3.0));
        assertEquals("0.5", Json.js(0.5));
        assertEquals("NaN", Json.js(Double.NaN));
        assertEquals("-Infinity", Json.js(Double.NEGATIVE_INFINITY));
        assertEquals("true", Json.js(true));
        assertEquals("Format C:", Json.js("Format C:"));
        assertEquals("[object Object]", Json.js(Map.of("a", 1.0)));
        assertEquals("1,,x", Json.js(Arrays.asList(1.0, null, "x")));
    }

    @Test
    void knowsWhatJavaScriptCountsAsTrue() {
        for (Object no : new Object[] {null, Json.UNDEFINED, false, 0.0, -0.0, Double.NaN, ""}) assertFalse(Json.truthy(no), String.valueOf(no));
        for (Object yes : new Object[] {true, 1.0, -1.0, "0", List.of(), Map.of()}) assertTrue(Json.truthy(yes), String.valueOf(yes));
    }
}
