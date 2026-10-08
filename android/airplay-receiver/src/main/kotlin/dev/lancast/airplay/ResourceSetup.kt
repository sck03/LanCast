package dev.lancast.airplay

/** Keep ownership local until setup succeeds, including failures after partial initialization. */
internal inline fun <T> T.configureOrRelease(release: (T) -> Unit, configure: (T) -> Unit): T {
    try {
        configure(this)
        return this
    } catch (failure: Throwable) {
        try { release(this) }
        catch (cleanup: Throwable) { if (cleanup !== failure) failure.addSuppressed(cleanup) }
        throw failure
    }
}
