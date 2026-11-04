package org.musicorganiser.bitwig;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.regex.Pattern;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;

/**
 * The extension's version is kept in two places: the pom (which the built extension's manifest takes, and Settings reads
 * to say which one Install puts in) and the version it tells Bitwig and the app. Settings compares the two, so they must
 * be raised together.
 */
class VersionTest {
    @Test
    @DisplayName("says the version the pom gives, which Settings reads from the built extension")
    void sameAsThePom() throws IOException {
        var pom = Files.readString(Path.of("pom.xml"));
        var project = Pattern.compile("<artifactId>music-organiser-bitwig</artifactId>\\s*<version>([^<]+)</version>").matcher(pom);
        assertTrue(project.find(), "pom.xml gives the project's version after its artifactId");
        assertEquals(project.group(1).trim(), MusicOrganiserExtensionDefinition.VERSION,
                     "raise MusicOrganiserExtensionDefinition.VERSION and the pom's version together");
    }
}
