package dev.lancast.control

/** Host visibility and system pickers own the same core that owns discovery results. */
class HostCoreLease {
    var visible = false
    var externalRequest = false
    fun releaseIfIdle(active: Boolean, release: () -> Unit) {
        if (!visible && !externalRequest && !active) release()
    }
}
