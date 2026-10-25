package dev.lancast.sender

/** Codec and audio owners release independently; a timed join never completes their ownership. */
internal class CaptureCompletion {
    private val lock = Any()
    private var codecDone = false
    private var audioDone = false
    private val callbacks = mutableListOf<() -> Unit>()
    fun codecFinished() = finish(codec = true)
    fun audioFinished() = finish(codec = false)
    private fun finish(codec: Boolean) {
        val ready = synchronized(lock) {
            if (codec) codecDone = true else audioDone = true
            if (codecDone && audioDone) callbacks.toList().also { callbacks.clear() } else emptyList()
        }
        ready.forEach { runCatching { it() } }
    }
    fun whenComplete(callback: () -> Unit) {
        val done = synchronized(lock) {
            if (codecDone && audioDone) true else { callbacks += callback; false }
        }
        if (done) callback()
    }
}
