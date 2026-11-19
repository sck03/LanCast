package dev.lancast.control

import org.junit.Assert.*
import org.junit.Test

class HostCoreLeaseTest {
    @Test fun documentPickerRetainsCoreAndOrdinaryBackgroundInvalidatesSelection() {
        val lease = HostCoreLease()
        var selected: String? = "tv"
        var coreAlive = true
        val release = { coreAlive = false; selected = null }
        lease.visible = true
        lease.externalRequest = true
        lease.visible = false
        lease.releaseIfIdle(false, release)
        assertTrue(coreAlive); assertEquals("tv", selected)
        lease.visible = true; lease.externalRequest = false
        lease.releaseIfIdle(false, release)
        assertTrue(coreAlive)
        lease.visible = false
        lease.releaseIfIdle(false, release)
        assertFalse(coreAlive); assertNull(selected)
    }
    @Test fun activeMediaRetainsCoreWhenHostIsHidden() {
        val lease = HostCoreLease()
        lease.releaseIfIdle(true) { fail("Active capture lost its core") }
    }
}
