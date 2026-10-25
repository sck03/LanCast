package dev.lancast.sender

import org.junit.Assert.*
import org.junit.Test
import java.util.concurrent.CountDownLatch
import java.util.concurrent.atomic.AtomicInteger

class CaptureCompletionTest {
    @Test fun codecCleanupDoesNotCompleteAStillRunningAudioOwner() {
        val completion = CaptureCompletion()
        var calls = 0
        completion.whenComplete { calls++ }
        completion.codecFinished(); completion.codecFinished()
        assertEquals(0, calls)
        completion.audioFinished(); completion.audioFinished()
        assertEquals(1, calls)
        completion.whenComplete { calls++ }
        assertEquals(2, calls)
    }
    @Test fun independentOwnersAndObserversCompleteExactlyOnceAcrossRaces() {
        val completion = CaptureCompletion()
        val start = CountDownLatch(1)
        val calls = AtomicInteger()
        val owners = listOf({ completion.codecFinished() }, { completion.audioFinished() })
        val threads = owners.map { done -> Thread { start.await(); repeat(100) { done() } }.apply { start() } }
        completion.whenComplete { calls.incrementAndGet() }
        start.countDown(); threads.forEach { it.join() }
        assertEquals(1, calls.get())
    }
}
