package dev.lancast.sender

import java.nio.ByteBuffer
import org.junit.Assert.*
import org.junit.Test

class CodecBuffersTest {
    @Test fun keyFrameCopiesOnlyTheValidSliceWithoutChangingCodecState() {
        val buffer = ByteBuffer.allocateDirect(8).apply {
            put(byteArrayOf(9, 8, 7, 6, 5, 4, 3, 2)); position(1); limit(7)
        }.asReadOnlyBuffer()
        val prefix = byteArrayOf(0, 0, 1)
        assertArrayEquals(byteArrayOf(0, 0, 1, 7, 6, 5), encodedFrame(buffer, 2, 3, prefix))
        assertEquals(1, buffer.position())
        assertEquals(7, buffer.limit())
        assertArrayEquals(byteArrayOf(0, 0, 1), prefix)
    }

    @Test fun ordinaryAndEmptyFramesDoNotRequireConfiguration() {
        val buffer = ByteBuffer.wrap(byteArrayOf(1, 2, 3))
        assertArrayEquals(byteArrayOf(2, 3), encodedFrame(buffer, 1, 2))
        assertArrayEquals(byteArrayOf(), encodedFrame(buffer, 3, 0))
    }

    @Test fun configurationUsesRemainingBytesAndAcceptsMissingChunks() {
        val sps = ByteBuffer.wrap(byteArrayOf(9, 1, 2, 8)).apply { position(1); limit(3) }
        val pps = ByteBuffer.wrap(byteArrayOf(3, 4)).asReadOnlyBuffer()
        assertArrayEquals(byteArrayOf(1, 2, 3, 4), codecConfiguration(sps, null, pps))
        assertEquals(1, sps.position())
        assertEquals(0, pps.position())
        assertArrayEquals(byteArrayOf(), codecConfiguration(null))
    }

    @Test fun invalidCodecRangesAreRejectedBeforeReading() {
        val buffer = ByteBuffer.allocate(10).apply { limit(4) }
        for ((offset, size) in listOf(-1 to 2, 0 to -1, 3 to 2, Int.MAX_VALUE to 1, 1 to Int.MAX_VALUE)) {
            assertThrows(IllegalArgumentException::class.java) { encodedFrame(buffer, offset, size) }
        }
    }
}
