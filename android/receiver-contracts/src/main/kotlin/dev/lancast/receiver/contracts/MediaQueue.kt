package dev.lancast.receiver.contracts

/** Bounded encoded data; offer never blocks the network or UI thread. */
class MediaQueue<T>(private val maxFrames: Int, private val maxBytes: Int, private val maxSpanUs: Long) {
    private data class Entry<T>(val item: T, val bytes: Int, val pts: Long)
    private val queue = java.util.ArrayDeque<Entry<T>>()
    private var bytes = 0
    @Synchronized fun offer(item: T, size: Int, pts: Long): Boolean {
        if (size < 0 || size > maxBytes || queue.size >= maxFrames || bytes > maxBytes - size) return false
        if (queue.isNotEmpty() && pts > 0 && queue.first.pts > 0 && pts - queue.first.pts > maxSpanUs) return false
        queue.addLast(Entry(item, size, pts)); bytes += size
        return true
    }
    @Synchronized fun poll(): T? = if (queue.isEmpty()) null else queue.removeFirst().let { bytes -= it.bytes; it.item }
    @Synchronized fun clear() { queue.clear(); bytes = 0 }
    @Synchronized fun size() = queue.size
}
