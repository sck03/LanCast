package dev.lancast.receiver.contracts

import org.junit.Assert.*
import org.junit.Test

class ReceiverContractsTest {
    @Test fun pendingConfirmationExcludesOtherProtocolsAndStaleStopCannotReleaseNewOwner() {
        val slots = ReceiverLease()
        val first = slots.acquire(Source.AIRPLAY, "1")!!
        assertNull(slots.acquire(Source.LANCAST, "2"))
        assertTrue(slots.release(first))
        val next = slots.acquire(Source.LANCAST, "2")!!
        assertFalse(slots.release(first))
        assertTrue(slots.owns(next))
    }
    @Test fun lateNetworkAndReadyCallbacksCannotResurrectDisabledService() {
        val cycle = EnableCycle(); val enabled = cycle.enable()!!
        assertTrue(cycle.networkLost(enabled))
        val stopped = cycle.stop()
        assertFalse(cycle.listening(enabled)); assertNull(cycle.enable())
        cycle.stopped(stopped); assertEquals(ServicePhase.DISABLED, cycle.phase)
        assertFalse(cycle.networkLost(enabled)); assertNotNull(cycle.enable())
    }
    @Test fun mediaLimitsIncludeBytesDurationAndFrameCount() {
        val q = MediaQueue<Int>(3, 10, 200)
        assertTrue(q.offer(1, 4, 100)); assertFalse(q.offer(2, 7, 110))
        assertFalse(q.offer(2, 1, 301)); assertTrue(q.offer(2, 4, 120))
        assertEquals(1, q.poll()); q.clear(); assertEquals(0, q.size())
        repeat(3) { assertTrue(q.offer(it, 1, 1)) }; assertFalse(q.offer(4, 1, 1))
    }
}
