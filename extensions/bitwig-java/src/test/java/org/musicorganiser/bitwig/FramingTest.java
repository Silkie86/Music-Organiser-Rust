package org.musicorganiser.bitwig;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.io.ByteArrayOutputStream;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import org.junit.jupiter.api.Test;

/** How messages travel between the app and the extension (see the app's BitwigLink.ReadMessageAsync). */
class FramingTest {
    @Test
    void messagesForTheAppArePlainAsciiEndingInANewline() {
        byte[] bytes = Framing.message(Json.write(Map.of("name", "Café ☕ 𝄞")));
        String text = new String(bytes, StandardCharsets.US_ASCII);
        assertEquals("{\"name\":\"Caf\\u00e9 \\u2615 \\ud834\\udd1e\"}\n", text);
        for (byte b : bytes) assertTrue(b >= 0, "one byte for each character");
        assertEquals(Map.of("name", "Café ☕ 𝄞"), Json.parse(text.trim()));
    }

    @Test
    void escapesAnythingOutsideAsciiEvenIfTheJsonDidnt() {
        assertEquals("\"\\u00fc\\u007f\"\n", new String(Framing.message("\"ü\u007f\""), StandardCharsets.US_ASCII));
    }

    @Test
    void readsRequestsAsUtf8() {
        byte[] request = "{\"cmd\":\"insert\",\"path\":\"C:\\\\Samples\\\\Café\\\\Kick ß.wav\"}".getBytes(StandardCharsets.UTF_8);
        Map<?, ?> map = (Map<?, ?>) Json.parse(Framing.text(request));
        assertEquals("C:\\Samples\\Café\\Kick ß.wav", map.get("path"));
        assertEquals("𝄞", Framing.text("𝄞".getBytes(StandardCharsets.UTF_8)));
    }

    @Test
    void putsTheLengthInFrontBigEndian() {
        byte[] framed = Framing.frame("{}\n".getBytes(StandardCharsets.US_ASCII));
        assertArrayEquals(new byte[] {0, 0, 0, 3, '{', '}', '\n'}, framed);
        byte[] big = Framing.frame(new byte[0x010203]);
        assertArrayEquals(new byte[] {0, 1, 2, 3}, new byte[] {big[0], big[1], big[2], big[3]});
    }

    @Test
    void takesMessagesApartHoweverTheBytesArrive() {
        List<String> sent = List.of("{\"id\":1}", "", "{\"cmd\":\"project\",\"name\":\"" + "x".repeat(70000) + "\"}", "{\"é\":1}");
        ByteArrayOutputStream stream = new ByteArrayOutputStream();
        for (String s : sent) stream.writeBytes(Framing.frame(s.getBytes(StandardCharsets.UTF_8)));
        byte[] all = stream.toByteArray();

        for (int chunk : new int[] {1, 3, 4, 5, 1000, all.length}) {
            Framing.Deframer deframer = new Framing.Deframer();
            List<String> received = new ArrayList<>();
            for (int at = 0; at < all.length; at += chunk)
                for (byte[] m : deframer.add(all, at, Math.min(chunk, all.length - at))) received.add(Framing.text(m));
            assertEquals(sent, received, "in chunks of " + chunk);
            assertEquals(0, deframer.pending());
        }
    }

    @Test
    void waitsForTheRestOfAMessage() {
        Framing.Deframer deframer = new Framing.Deframer();
        byte[] framed = Framing.frame("hello".getBytes(StandardCharsets.US_ASCII));
        assertEquals(List.of(), deframer.add(framed, 0, 6));
        assertEquals(6, deframer.pending());
        List<byte[]> done = deframer.add(framed, 6, framed.length - 6);
        assertEquals(1, done.size());
        assertEquals("hello", Framing.text(done.get(0)));
    }

    @Test
    void refusesALengthNoMessageHas() {
        Framing.Deframer deframer = new Framing.Deframer();
        assertThrows(IllegalStateException.class, () -> deframer.add(new byte[] {(byte) 0xff, 0, 0, 0}));
        assertThrows(IllegalStateException.class, () -> new Framing.Deframer().add(new byte[] {0x7f, 0, 0, 0}));
    }
}
