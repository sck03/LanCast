package dev.lancast.receiver.contracts

import java.util.ArrayDeque
import java.util.concurrent.TimeUnit
import java.util.concurrent.locks.ReentrantLock
import kotlin.concurrent.withLock

/** Bounded encoded data. Producers never wait for space; closing discards data and wakes consumers. */
class MediaQueue<T : Any>(private val maxFrames: Int, private val maxBytes: Int, private val maxSpanUs: Long) : AutoCloseable {
    init { require(maxFrames > 0 && maxBytes >= 0 && maxSpanUs >= 0) }
    private class Entry<T>(val item: T, val bytes: Int, val pts: Long)
    private val lock = ReentrantLock()
    private val available = lock.newCondition()
    private val queue = ArrayDeque<Entry<T>>()
    // Monotonic deques track the full span in amortized O(1), including reordered timestamps.
    // A zero timestamp denotes configuration and does not reset or mask the media budget.
    private val earliest = ArrayDeque<Entry<T>>()
    private val latest = ArrayDeque<Entry<T>>()
    private var bytes = 0
    private var closed = false

    fun offer(item: T, size: Int, pts: Long): Boolean = lock.withLock {
        if (closed || size < 0 || size > maxBytes || pts < 0 || queue.size >= maxFrames || bytes > maxBytes - size) return false
        if (pts > 0) {
            val first = minOf(pts, earliest.peekFirst()?.pts ?: pts)
            val last = maxOf(pts, latest.peekFirst()?.pts ?: pts)
            if (last - first > maxSpanUs) return false
        }
        val entry = Entry(item, size, pts)
        queue.addLast(entry)
        bytes += size
        if (pts > 0) {
            while (earliest.isNotEmpty() && earliest.last.pts > pts) earliest.removeLast()
            while (latest.isNotEmpty() && latest.last.pts < pts) latest.removeLast()
            earliest.addLast(entry)
            latest.addLast(entry)
        }
        available.signal()
        true
    }

    /** Wait indefinitely for input, or return null when closed. Interruptible by the owner. */
    fun take(): T? = lock.withLock {
        while (queue.isEmpty() && !closed) available.await()
        removeFirst()
    }

    /** A finite wait also lets codec workers drain output when no new input arrives. */
    fun poll(timeoutMillis: Long = 0): T? = lock.withLock {
        require(timeoutMillis >= 0)
        var remaining = TimeUnit.MILLISECONDS.toNanos(timeoutMillis)
        while (queue.isEmpty() && !closed && remaining > 0) remaining = available.awaitNanos(remaining)
        removeFirst()
    }

    private fun removeFirst(): T? {
        val entry = queue.pollFirst() ?: return null
        bytes -= entry.bytes
        if (earliest.peekFirst() === entry) earliest.removeFirst()
        if (latest.peekFirst() === entry) latest.removeFirst()
        return entry.item
    }

    private fun clearLocked() {
        queue.clear()
        earliest.clear()
        latest.clear()
        bytes = 0
    }

    fun clear() = lock.withLock { clearLocked() }
    fun size() = lock.withLock { queue.size }
    override fun close() = lock.withLock {
        closed = true
        clearLocked()
        available.signalAll()
    }
}
