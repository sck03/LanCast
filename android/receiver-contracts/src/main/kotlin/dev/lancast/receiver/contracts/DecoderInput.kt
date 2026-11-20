package dev.lancast.receiver.contracts

/** Retains one input while output is drained. The deadline never resets on a retry. */
class DecoderInput<T>(private val timeoutNs: Long) {
    var pending: T? = null
        private set
    private var deadline = 0L
    fun defer(item: T, now: Long) {
        if (pending == null) { pending = item; deadline = now + timeoutNs }
        checkDeadline(now)
    }
    fun checkDeadline(now: Long) {
        check(pending == null || now < deadline) { "解码器持续拥塞，无法继续播放" }
    }
    fun accepted() { pending = null }
}
