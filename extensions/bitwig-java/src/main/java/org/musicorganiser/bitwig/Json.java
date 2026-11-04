package org.musicorganiser.bitwig;

import java.math.BigDecimal;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Iterator;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * A small JSON reader and writer, so the extension ships alone with nothing but the JDK behind it.
 *
 * It reads JSON as JavaScript's JSON.parse does (strictly), into {@code Map} (keys in order), {@code List},
 * {@code String}, {@code Double}, {@code Boolean} and {@code null}, and writes those (and any other
 * {@code Number}, arrays and iterables) back as JSON.stringify would: numbers as JavaScript writes them, and
 * NaN and the infinities as null. Everything outside ASCII is written as a \\u escape, which keeps the link's
 * messages and the report files plain ASCII.
 *
 * The link was a JavaScript script first, and the app relies on its error messages, so {@link #js} gives a
 * value as JavaScript's String() would (with {@link #UNDEFINED} for a field a request left out).
 */
final class Json {
    private Json() {}

    /** Thrown for text that isn't JSON. */
    static final class JsonException extends RuntimeException {
        private static final long serialVersionUID = 1L;

        JsonException(String message) {
            super(message);
        }
    }

    /** A field a request didn't have, which JavaScript calls undefined (and writes so in messages). */
    static final Object UNDEFINED = new Object() {
        @Override
        public String toString() {
            return "undefined";
        }
    };

    /** Deep enough for anything the app sends, shallow enough that odd input can't run the stack out. */
    private static final int MAX_DEPTH = 256;

    // ---- Reading ----

    /** Reads one JSON value (with nothing after it but white space). */
    static Object parse(String text) {
        Reader reader = new Reader(text);
        reader.space();
        Object value = reader.value(0);
        reader.space();
        if (reader.at < text.length()) throw reader.fail("Unexpected text after the JSON");
        return value;
    }

    private static final class Reader {
        final String text;
        int at;

        Reader(String text) {
            this.text = text;
        }

        JsonException fail(String what) {
            return new JsonException(what + " at character " + at + ".");
        }

        void space() {
            while (at < text.length()) {
                char c = text.charAt(at);
                if (c != ' ' && c != '\t' && c != '\n' && c != '\r') break;
                at++;
            }
        }

        Object value(int depth) {
            if (depth > MAX_DEPTH) throw fail("Nested too deeply");
            if (at >= text.length()) throw fail("Unexpected end of the JSON");
            char c = text.charAt(at);
            switch (c) {
                case '{': return object(depth);
                case '[': return array(depth);
                case '"': return string();
                case 't': return word("true", Boolean.TRUE);
                case 'f': return word("false", Boolean.FALSE);
                case 'n': return word("null", null);
                default:
                    if (c == '-' || (c >= '0' && c <= '9')) return number();
                    throw fail("Unexpected '" + c + "'");
            }
        }

        Object word(String word, Object value) {
            if (!text.startsWith(word, at)) throw fail("Unexpected '" + text.charAt(at) + "'");
            at += word.length();
            return value;
        }

        Map<String, Object> object(int depth) {
            Map<String, Object> map = new LinkedHashMap<>();
            at++;
            space();
            if (at < text.length() && text.charAt(at) == '}') {
                at++;
                return map;
            }
            while (true) {
                space();
                if (at >= text.length() || text.charAt(at) != '"') throw fail("Expected a name in quotes");
                String key = string();
                space();
                if (at >= text.length() || text.charAt(at) != ':') throw fail("Expected ':'");
                at++;
                space();
                map.put(key, value(depth + 1));
                space();
                if (at >= text.length()) throw fail("Unexpected end of the JSON");
                char c = text.charAt(at++);
                if (c == '}') return map;
                if (c != ',') {
                    at--;
                    throw fail("Expected ',' or '}'");
                }
            }
        }

        List<Object> array(int depth) {
            List<Object> list = new ArrayList<>();
            at++;
            space();
            if (at < text.length() && text.charAt(at) == ']') {
                at++;
                return list;
            }
            while (true) {
                space();
                list.add(value(depth + 1));
                space();
                if (at >= text.length()) throw fail("Unexpected end of the JSON");
                char c = text.charAt(at++);
                if (c == ']') return list;
                if (c != ',') {
                    at--;
                    throw fail("Expected ',' or ']'");
                }
            }
        }

        String string() {
            at++;   // the opening quote
            StringBuilder out = new StringBuilder();
            while (true) {
                if (at >= text.length()) throw fail("Unterminated string");
                char c = text.charAt(at++);
                if (c == '"') return out.toString();
                if (c < 0x20) {
                    at--;
                    throw fail("Bad control character in string");
                }
                if (c != '\\') {
                    out.append(c);
                    continue;
                }
                if (at >= text.length()) throw fail("Unterminated string");
                char e = text.charAt(at++);
                switch (e) {
                    case '"': out.append('"'); break;
                    case '\\': out.append('\\'); break;
                    case '/': out.append('/'); break;
                    case 'b': out.append('\b'); break;
                    case 'f': out.append('\f'); break;
                    case 'n': out.append('\n'); break;
                    case 'r': out.append('\r'); break;
                    case 't': out.append('\t'); break;
                    case 'u':
                        if (at + 4 > text.length()) throw fail("Bad \\u escape");
                        int code = 0;
                        for (int i = 0; i < 4; i++) {
                            int digit = Character.digit(text.charAt(at + i), 16);
                            if (digit < 0) throw fail("Bad \\u escape");
                            code = code * 16 + digit;
                        }
                        at += 4;
                        out.append((char) code);
                        break;
                    default:
                        at--;
                        throw fail("Bad escape");
                }
            }
        }

        Double number() {
            int start = at;
            if (text.charAt(at) == '-') at++;
            if (at < text.length() && text.charAt(at) == '0') {
                at++;
            } else if (!digits()) {
                throw fail("Bad number");
            }
            if (at < text.length() && text.charAt(at) == '.') {
                at++;
                if (!digits()) throw fail("Bad number");
            }
            if (at < text.length() && (text.charAt(at) == 'e' || text.charAt(at) == 'E')) {
                at++;
                if (at < text.length() && (text.charAt(at) == '+' || text.charAt(at) == '-')) at++;
                if (!digits()) throw fail("Bad number");
            }
            return Double.parseDouble(text.substring(start, at));
        }

        boolean digits() {
            int start = at;
            while (at < text.length() && text.charAt(at) >= '0' && text.charAt(at) <= '9') at++;
            return at > start;
        }
    }

    // ---- Writing ----

    /** The value as compact JSON, as the link sends it. */
    static String write(Object value) {
        StringBuilder out = new StringBuilder();
        write(out, value, null, "");
        return out.toString();
    }

    /** The value as JSON indented by two spaces, for files people read. */
    static String pretty(Object value) {
        StringBuilder out = new StringBuilder();
        write(out, value, "  ", "");
        return out.toString();
    }

    private static void write(StringBuilder out, Object value, String indent, String at) {
        if (value == null || value == UNDEFINED) {
            out.append("null");
        } else if (value instanceof CharSequence || value instanceof Character) {
            quote(out, value.toString());
        } else if (value instanceof Boolean) {
            out.append(value);
        } else if (value instanceof Double || value instanceof Float) {
            double d = ((Number) value).doubleValue();
            out.append(Double.isFinite(d) ? number(value instanceof Float ? Double.parseDouble(value.toString()) : d) : "null");
        } else if (value instanceof Number) {
            out.append(value);
        } else if (value instanceof Map) {
            Map<?, ?> map = (Map<?, ?>) value;
            String inner = indent == null ? null : at + indent;
            out.append('{');
            boolean first = true;
            for (Map.Entry<?, ?> entry : map.entrySet()) {
                if (entry.getValue() == UNDEFINED) continue;   // as JSON.stringify leaves undefined out
                if (!first) out.append(',');
                first = false;
                if (inner != null) out.append('\n').append(inner);
                quote(out, String.valueOf(entry.getKey()));
                out.append(inner != null ? ": " : ":");
                write(out, entry.getValue(), indent, inner == null ? at : inner);
            }
            if (!first && inner != null) out.append('\n').append(at);
            out.append('}');
        } else if (value instanceof Iterable || value instanceof Object[]) {
            Iterator<?> items = value instanceof Iterable ? ((Iterable<?>) value).iterator() : Arrays.asList((Object[]) value).iterator();
            String inner = indent == null ? null : at + indent;
            out.append('[');
            boolean first = true;
            while (items.hasNext()) {
                if (!first) out.append(',');
                first = false;
                if (inner != null) out.append('\n').append(inner);
                write(out, items.next(), indent, inner == null ? at : inner);
            }
            if (!first && inner != null) out.append('\n').append(at);
            out.append(']');
        } else {
            throw new IllegalArgumentException("Can't write a " + value.getClass().getName() + " as JSON.");
        }
    }

    private static void quote(StringBuilder out, String text) {
        out.append('"');
        for (int i = 0; i < text.length(); i++) {
            char c = text.charAt(i);
            switch (c) {
                case '"': out.append("\\\""); break;
                case '\\': out.append("\\\\"); break;
                case '\b': out.append("\\b"); break;
                case '\f': out.append("\\f"); break;
                case '\n': out.append("\\n"); break;
                case '\r': out.append("\\r"); break;
                case '\t': out.append("\\t"); break;
                default:
                    if (c < 0x20 || c >= 0x7f) unicode(out, c);
                    else out.append(c);
            }
        }
        out.append('"');
    }

    /** \\u and four lower-case hex digits, as JavaScript writes them. */
    static void unicode(StringBuilder out, char c) {
        String hex = Integer.toHexString(c);
        out.append("\\u");
        for (int n = hex.length(); n < 4; n++) out.append('0');
        out.append(hex);
    }

    /**
     * A finite number as JavaScript writes it: whole numbers without a point, the shortest digits that read
     * back as the same number, and an exponent only for very big or very small ones (1e+21, 1e-7).
     */
    static String number(double d) {
        if (d == 0) return "0";   // -0 too
        // Double.toString gives the shortest digits; only where they go differs from JavaScript.
        BigDecimal exact = new BigDecimal(Double.toString(Math.abs(d))).stripTrailingZeros();
        String digits = exact.unscaledValue().toString();
        int k = digits.length();
        int n = k - exact.scale();   // the number is 0.<digits> times 10 to the n
        StringBuilder out = new StringBuilder();
        if (d < 0) out.append('-');
        if (k <= n && n <= 21) {
            out.append(digits);
            for (int i = k; i < n; i++) out.append('0');
        } else if (0 < n && n <= 21) {
            out.append(digits, 0, n).append('.').append(digits, n, k);
        } else if (-6 < n && n <= 0) {
            out.append("0.");
            for (int i = n; i < 0; i++) out.append('0');
            out.append(digits);
        } else {
            out.append(digits.charAt(0));
            if (k > 1) out.append('.').append(digits, 1, k);
            int e = n - 1;
            out.append('e').append(e < 0 ? '-' : '+').append(Math.abs(e));
        }
        return out.toString();
    }

    // ---- As JavaScript sees it ----

    /** The value as JavaScript's String() writes it, for messages that show what a request asked for. */
    static String js(Object value) {
        if (value == null) return "null";
        if (value == UNDEFINED) return "undefined";
        if (value instanceof Double || value instanceof Float) {
            double d = ((Number) value).doubleValue();
            if (Double.isNaN(d)) return "NaN";
            if (Double.isInfinite(d)) return d > 0 ? "Infinity" : "-Infinity";
            return number(d);
        }
        if (value instanceof Map) return "[object Object]";
        if (value instanceof List) {
            StringBuilder out = new StringBuilder();
            List<?> list = (List<?>) value;
            for (int i = 0; i < list.size(); i++) {
                if (i > 0) out.append(',');
                Object item = list.get(i);
                if (item != null && item != UNDEFINED) out.append(js(item));
            }
            return out.toString();
        }
        return String.valueOf(value);
    }

    /** Whether JavaScript counts the value as true: anything but null, undefined, false, 0, NaN and "". */
    static boolean truthy(Object value) {
        if (value == null || value == UNDEFINED) return false;
        if (value instanceof Boolean) return (Boolean) value;
        if (value instanceof Number) {
            double d = ((Number) value).doubleValue();
            return d != 0 && !Double.isNaN(d);
        }
        if (value instanceof CharSequence) return ((CharSequence) value).length() > 0;
        return true;
    }
}
