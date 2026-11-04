package org.musicorganiser.bitwig;

import com.bitwig.extension.api.PlatformType;
import com.bitwig.extension.controller.AutoDetectionMidiPortNamesList;
import com.bitwig.extension.controller.ControllerExtension;
import com.bitwig.extension.controller.ControllerExtensionDefinition;
import com.bitwig.extension.controller.api.ControllerHost;
import java.util.UUID;

/**
 * Tells Bitwig about the extension (META-INF/services lists this class). It keeps the JavaScript script's
 * identity, above all its ID, so Bitwig treats it as the same controller: one already added in Settings >
 * Controllers stays added, with its "Let the app make changes" setting.
 */
public final class MusicOrganiserExtensionDefinition extends ControllerExtensionDefinition {
    /** The ID the JavaScript script (extensions/bitwig/MusicOrganiser.control.js) was defined with. */
    static final UUID ID = UUID.fromString("ed35e3c7-c544-49ed-936d-c236b0c9b73b");
    static final String VERSION = "3.18";
    /**
     * API 23 (Bitwig 6.1) has everything the link uses: parameter steps and names, origins, automation, channel
     * IDs, the last clicked parameter and the master recorder. Nothing newer (and no beta method) is used.
     */
    static final int API_VERSION = 23;
    /**
     * The loopMIDI port the link's MIDI in and out are set to, for its loopback. It's not "Music Organiser", the port the FL
     * Studio link uses: FL Studio must never see what the Bitwig link sends itself.
     */
    static final String LOOPBACK_PORT = "Music Organiser Bitwig";

    @Override
    public String getName() {
        return "Music Organiser";
    }

    @Override
    public String getAuthor() {
        return "Music Organiser";
    }

    @Override
    public String getVersion() {
        return VERSION;
    }

    @Override
    public UUID getId() {
        return ID;
    }

    @Override
    public int getRequiredAPIVersion() {
        return API_VERSION;
    }

    @Override
    public String getHardwareVendor() {
        return "Music Organiser";
    }

    @Override
    public String getHardwareModel() {
        return "DAW link";
    }

    /**
     * One MIDI in port and one MIDI out port, only for the link's loopback (see Link's command routes): the link sends
     * itself MIDI through the out port, and Bitwig hands it back on the in port as if a device had sent it, since Bitwig
     * may carry out some commands (Save, say) only while it handles input from a device. Both have to be the loopMIDI port
     * called "Music Organiser Bitwig" ({@link #LOOPBACK_PORT}), never "Music Organiser", which is the FL Studio link's: FL
     * Studio must never see these messages. Only the routes that go through MIDI send anything through them, and Bitwig
     * 6.1.3 starts the extension, and the link answers, with them unassigned, but it has only saved for the link once they
     * were set (from its flush, and once straight away; the flush was never tried without them): see "Routes and the MIDI
     * loopback" in extensions/README.md.
     */
    @Override
    public int getNumMidiInPorts() {
        return 1;
    }

    @Override
    public int getNumMidiOutPorts() {
        return 1;
    }

    /** Offers the loopback port for both, on every platform, so Bitwig can fill them in when it finds the port. */
    @Override
    public void listAutoDetectionMidiPortNames(AutoDetectionMidiPortNamesList list, PlatformType platformType) {
        list.add(new String[] {LOOPBACK_PORT}, new String[] {LOOPBACK_PORT});
    }

    @Override
    public ControllerExtension createInstance(ControllerHost host) {
        return new MusicOrganiserExtension(this, host);
    }
}
