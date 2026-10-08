package dev.lancast.receiver.contracts

import org.junit.Assert.*
import org.junit.Test
import java.util.Random
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.TimeoutException

class MediaQueueTest {
    @Test fun formatAtHeadCannotBypassTimeBudget() {
        val queue = MediaQueue<String>(8, 100, 200)
        assertTrue(queue.offer("format", 4, 0))
        assertTrue(queue.offer("first", 4, 100))
        assertFalse(queue.offer("too late", 4, 301))
        assertEquals("format", queue.poll())
        assertFalse(queue.offer("still too late", 4, 301))
    }

    @Test fun reorderedTimestampsUseTheEntireQueuedSpan() {
        val queue = MediaQueue<String>(8, 100, 200)
        assertTrue(queue.offer("first", 4, 300))
        assertTrue(queue.offer("reordered", 4, 100))
        assertFalse(queue.offer("too early", 4, 99))
        assertFalse(queue.offer("too late", 4, 301))
    }

    @Test fun removingExtremaAndClearingRestoresTheBudget() {
        val queue = MediaQueue<Int>(8, 100, 200)
        assertTrue(queue.offer(1, 4, 300))
        assertTrue(queue.offer(2, 4, 100))
        assertTrue(queue.offer(3, 4, 100))
        assertEquals(1, queue.poll())
        assertTrue(queue.offer(4, 4, 1))
        assertFalse(queue.offer(5, 4, 302))
        queue.clear()
        assertTrue(queue.offer(6, 100, Long.MAX_VALUE))
        assertEquals(6, queue.poll())
        assertEquals(0, queue.size())
    }

    @Test fun allBoundsMatchAReferenceWindowAcrossReorderingAndRemoval() {
        data class Frame(val id: Int, val size: Int, val pts: Long)
        val queue = MediaQueue<Int>(7, 30, 20)
        val expected = mutableListOf<Frame>()
        val random = Random(23)
        repeat(10_000) { id ->
            when (random.nextInt(10)) {
                0 -> { queue.clear(); expected.clear() }
                1, 2 -> assertEquals(expected.removeFirstOrNull()?.id, queue.poll())
                else -> {
                    val frame = Frame(id, random.nextInt(35) - 1, when (random.nextInt(10)) {
                        0 -> 0L
                        1 -> Long.MAX_VALUE - random.nextInt(40)
                        else -> random.nextInt(60).toLong() - 1
                    })
                    val timestamps = (expected + frame).map { it.pts }.filter { it > 0 }
                    val span = if (timestamps.isEmpty()) 0L else timestamps.max() - timestamps.min()
                    val accepted = frame.size >= 0 && frame.pts >= 0 && expected.size < 7 &&
                        expected.sumOf { it.size } + frame.size <= 30 && span <= 20
                    assertEquals("offer $id", accepted, queue.offer(frame.id, frame.size, frame.pts))
                    if (accepted) expected += frame
                }
            }
            assertEquals("size $id", expected.size, queue.size())
        }
        for (frame in expected) assertEquals(frame.id, queue.poll())
        assertNull(queue.poll())
    }

    @Test fun emptyConsumerWaitsUntilInputAndCloseWakesAllConsumers() {
        val queue = MediaQueue<Int>(3, 10, 200)
        val workers = Executors.newFixedThreadPool(2)
        try {
            val first = workers.submit<Int?> { queue.take() }
            assertThrows(TimeoutException::class.java) { first.get(30, TimeUnit.MILLISECONDS) }
            assertTrue(queue.offer(7, 1, 100))
            assertEquals(7, first.get(5, TimeUnit.SECONDS))
            val waiting = workers.submit<Int?> { queue.take() }
            val timed = workers.submit<Int?> { queue.poll(30_000) }
            queue.close()
            assertNull(waiting.get(5, TimeUnit.SECONDS))
            assertNull(timed.get(5, TimeUnit.SECONDS))
        } finally { queue.close(); workers.shutdownNow() }
    }

    @Test fun finiteWaitTimesOutOrWakesOnInput() {
        val queue = MediaQueue<Int>(3, 10, 200)
        val worker = Executors.newSingleThreadExecutor()
        try {
            assertNull(queue.poll(1))
            val waiting = worker.submit<Int?> { queue.poll(30_000) }
            assertThrows(TimeoutException::class.java) { waiting.get(30, TimeUnit.MILLISECONDS) }
            assertTrue(queue.offer(7, 1, 100))
            assertEquals(7, waiting.get(5, TimeUnit.SECONDS))
        } finally { queue.close(); worker.shutdownNow() }
    }

    @Test fun closeRacingWithOffersCannotRetainFramesOrReopen() {
        val worker = Executors.newSingleThreadExecutor()
        try {
            repeat(100) {
                val queue = MediaQueue<Int>(4, 20, 200)
                val start = CountDownLatch(1)
                val producer = worker.submit {
                    start.await()
                    repeat(100) { queue.offer(it, 2, 100) }
                }
                start.countDown()
                queue.close()
                producer.get(5, TimeUnit.SECONDS)
                queue.clear(); queue.close()
                assertEquals(0, queue.size())
                assertNull(queue.take())
                assertFalse(queue.offer(0, 0, 0))
            }
        } finally { worker.shutdownNow() }
    }

    @Test fun interruptingAConsumerDoesNotLoseInputOrCloseTheQueue() {
        val queue = MediaQueue<Int>(3, 10, 200)
        val worker = Executors.newSingleThreadExecutor()
        val entered = CountDownLatch(1)
        val interrupted = CountDownLatch(1)
        try {
            val waiting = worker.submit {
                entered.countDown()
                try { queue.take(); fail("take should be interrupted") }
                catch (_: InterruptedException) { interrupted.countDown() }
            }
            assertTrue(entered.await(5, TimeUnit.SECONDS))
            waiting.cancel(true)
            assertTrue(interrupted.await(5, TimeUnit.SECONDS))
            assertTrue(queue.offer(7, 1, 100))
            assertEquals(7, queue.take())
        } finally { queue.close(); worker.shutdownNow() }
    }

    @Test fun invalidLimitsAndInputsAreRejectedWithoutOverflow() {
        assertThrows(IllegalArgumentException::class.java) { MediaQueue<Int>(0, 10, 200) }
        assertThrows(IllegalArgumentException::class.java) { MediaQueue<Int>(1, -1, 200) }
        assertThrows(IllegalArgumentException::class.java) { MediaQueue<Int>(1, 10, -1) }
        val queue = MediaQueue<Int>(4, Int.MAX_VALUE, 200)
        assertFalse(queue.offer(1, -1, 1))
        assertFalse(queue.offer(1, 1, -1))
        assertTrue(queue.offer(1, Int.MAX_VALUE, Long.MAX_VALUE))
        assertFalse(queue.offer(2, 1, Long.MAX_VALUE))
        assertFalse(queue.offer(2, 0, 1))
        assertThrows(IllegalArgumentException::class.java) { queue.poll(-1) }
    }
}
