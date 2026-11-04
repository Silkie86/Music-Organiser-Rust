package org.musicorganiser.bitwig;

import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;

/**
 * How the link's messages travel. Bitwig's remote connections put a 4-byte big-endian length in front of
 * each message, both ways, and take it off again: the extension sends and receives the message alone.
 * Messages to the app are JSON in plain ASCII (anything else as \\u escapes) ending in a newline, so the app
 * can also read them without the length, as it did with the JavaScript script. Requests from the app are
 * UTF-8 JSON.
 *
 * {@link #frame} and {@link Deframer} do what Bitwig does on the wire, for stand-ins that play Bitwig's part.
 */
final class Framing {
    private Framing() {}

    /** The longest message a stand-in accepts, as the app does: anything longer is a broken stream. */
    static final int MAX_MESSAGE = 64 << 20;

    /** A message for the app: the JSON as ASCII, every character one byte, then a newline. */
    static byte[] message(String json) {
        StringBuilder ascii = new StringBuilder(json.length() + 1);
        for (int i = 0; i < json.length(); i++) {
            char c = json.charAt(i);
            // Outside ASCII a character can only be inside a JSON string, where an escape means the same.
            if (c >= 0x7f) Json.unicode(ascii, c);
            else ascii.append(c);
        }
        ascii.append('\n');
        return ascii.toString().getBytes(StandardCharsets.US_ASCII);
    }

    /** A request's bytes as text. */
    static String text(byte[] data) {
        return new String(data, StandardCharsets.UTF_8);
    }

    /** A message with its length in front, as Bitwig sends it on the wire. */
    static byte[] frame(byte[] message) {
        byte[] framed = new byte[4 + message.length];
        int n = message.length;
        framed[0] = (byte) (n >>> 24);
        framed[1] = (byte) (n >>> 16);
        framed[2] = (byte) (n >>> 8);
        framed[3] = (byte) n;
        System.arraycopy(message, 0, framed, 4, n);
        return framed;
    }

    /** Takes framed messages apart again, however the bytes arrive (a byte at a time, or several messages at once). */
    static final class Deframer {
        private byte[] buffer = new byte[256];
        private int length;

        /** The messages completed by these bytes, in order. */
        List<byte[]> add(byte[] bytes, int offset, int count) {
            if (length + count > buffer.length) buffer = Arrays.copyOf(buffer, Math.max(buffer.length * 2, length + count));
            System.arraycopy(bytes, offset, buffer, length, count);
            length += count;
            List<byte[]> messages = new ArrayList<>();
            int at = 0;
            while (length - at >= 4) {
                int n = ((buffer[at] & 0xff) << 24) | ((buffer[at + 1] & 0xff) << 16) | ((buffer[at + 2] & 0xff) << 8) | (buffer[at + 3] & 0xff);
                if (n < 0 || n > MAX_MESSAGE) throw new IllegalStateException("A message of " + (n & 0xffffffffL) + " bytes isn't one.");
                if (length - at - 4 < n) break;
                messages.add(Arrays.copyOfRange(buffer, at + 4, at + 4 + n));
                at += 4 + n;
            }
            if (at > 0) {
                System.arraycopy(buffer, at, buffer, 0, length - at);
                length -= at;
            }
            return messages;
        }

        List<byte[]> add(byte[] bytes) {
            return add(bytes, 0, bytes.length);
        }

        /** Bytes of a message still to come. */
        int pending() {
            return length;
        }
    }
}
