package dev.lancast.sender

import java.nio.ByteBuffer

/** Copy only the codec's valid range, prepending configuration in the same allocation. */
internal fun encodedFrame(buffer: ByteBuffer, offset: Int, size: Int, prefix: ByteArray? = null): ByteArray {
    require(offset >= 0 && size >= 0 && offset <= buffer.limit() - size)
    val prefixSize = prefix?.size ?: 0
    require(prefixSize <= Int.MAX_VALUE - size)
    return ByteArray(prefixSize + size).also { bytes ->
        prefix?.copyInto(bytes)
        buffer.duplicate().apply { position(offset); limit(offset + size) }.get(bytes, prefixSize, size)
    }
}

internal fun codecConfiguration(vararg chunks: ByteBuffer?): ByteArray {
    val buffers = chunks.filterNotNull().map { it.duplicate() }
    return ByteArray(buffers.sumOf { it.remaining() }).also { bytes ->
        var offset = 0
        for (buffer in buffers) {
            val size = buffer.remaining()
            buffer.get(bytes, offset, size)
            offset += size
        }
    }
}
