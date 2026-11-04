package org.musicorganiser.bitwig;

/** An error to report to the app as it is: its message is written for whoever uses the app. */
final class LinkError extends RuntimeException {
    private static final long serialVersionUID = 1L;

    LinkError(String message) {
        super(message, null, false, false);
    }
}
