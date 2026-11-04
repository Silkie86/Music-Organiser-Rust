package org.musicorganiser.bitwig;

import com.bitwig.extension.controller.ControllerExtension;
import com.bitwig.extension.controller.api.ControllerHost;
import java.time.Clock;

/** What Bitwig runs: hands everything to {@link Link}, which tests can also run with a stand-in Bitwig. */
public final class MusicOrganiserExtension extends ControllerExtension {
    private final Link link;

    MusicOrganiserExtension(MusicOrganiserExtensionDefinition definition, ControllerHost host) {
        super(definition, host);
        Clock clock = Clock.systemUTC();
        link = new Link(host, new Reports(Reports.defaultFolder(), clock), clock);
    }

    @Override
    public void init() {
        link.init();
    }

    @Override
    public void flush() {
        link.flush();
    }

    @Override
    public void exit() {
        link.exit();
    }
}
