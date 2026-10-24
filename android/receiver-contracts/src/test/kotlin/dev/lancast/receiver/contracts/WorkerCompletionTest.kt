package dev.lancast.receiver.contracts

import org.junit.Assert.*
import org.junit.Test
import java.util.concurrent.CountDownLatch
import java.util.concurrent.atomic.AtomicInteger

class WorkerCompletionTest {
    @Test fun ownershipIsHeldUntilEveryWorkerExitsAndDuplicateExitIsHarmless() {
        val leases = ReceiverLease()
        val lease = leases.acquire(Source.AIRPLAY, "old")
        val completion = WorkerCompletion(2)
        val video = completion.participant()
        val audio = completion.participant()
        completion.whenComplete { leases.release(lease) }
        video(); video()
        assertNull(leases.acquire(Source.LANCAST, "new"))
        audio()
        assertNotNull(leases.acquire(Source.LANCAST, "new"))
    }

    @Test fun concurrentExitAndLateObserversCompleteExactlyOnce() {
        val completion = WorkerCompletion(2)
        val first = completion.participant()
        val second = completion.participant()
        val start = CountDownLatch(1)
        val calls = AtomicInteger()
        completion.whenComplete { calls.incrementAndGet() }
        val threads = listOf(first, second).map { done -> Thread { start.await(); repeat(100) { done() } }.apply { start() } }
        start.countDown(); threads.forEach { it.join() }
        completion.whenComplete { calls.incrementAndGet() }
        assertEquals(2, calls.get())
    }
}
