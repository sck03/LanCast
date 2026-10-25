package dev.lancast.media

import org.junit.Assert.*
import org.junit.Test

class CaptureSizeTest {
    @Test fun highResolutionAndPortraitSourcesStayInsideTheNegotiatedBudget() {
        assertEquals(1280 to 720, fitCaptureSize(3840, 2160, 1280, 720))
        assertEquals(404 to 720, fitCaptureSize(1440, 2560, 1280, 720))
        assertEquals(640 to 480, fitCaptureSize(640, 480, 1280, 720))
    }
    @Test fun oddAndExtremeDimensionsRemainEvenPositiveAndBounded() {
        for ((width, height) in listOf(1 to 1, 1281 to 721, Int.MAX_VALUE to 1, 1 to Int.MAX_VALUE)) {
            val (w, h) = fitCaptureSize(width, height, 1280, 720)
            assertTrue(w in 2..1280 && h in 2..720)
            assertEquals(0, w % 2); assertEquals(0, h % 2)
        }
    }
}
