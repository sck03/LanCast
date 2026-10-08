package dev.lancast.airplay

import org.junit.Assert.*
import org.junit.Test

class ResourceSetupTest {
    @Test fun successfulSetupTransfersOwnershipWithoutReleasing() {
        val resource = Any()
        var initialized = false
        var releases = 0
        assertSame(resource, resource.configureOrRelease({ releases++ }) {
            assertSame(resource, it)
            initialized = true
        })
        assertTrue(initialized)
        assertEquals(0, releases)
    }

    @Test fun partialSetupFailureReleasesTheAcquiredResourceOnce() {
        val resource = Any()
        var configured = false
        var releases = 0
        val failure = IllegalStateException("start failed after configure")
        val thrown = assertThrows(IllegalStateException::class.java) {
            resource.configureOrRelease({
                assertSame(resource, it)
                assertTrue(configured)
                releases++
            }) {
                configured = true
                throw failure
            }
        }
        assertSame(failure, thrown)
        assertEquals(1, releases)
    }

    @Test fun releaseFailureDoesNotHideTheOriginalSetupFailure() {
        val failure = AssertionError("setup failed")
        val cleanup = IllegalStateException("release failed")
        val thrown = assertThrows(AssertionError::class.java) {
            Any().configureOrRelease({ throw cleanup }) { throw failure }
        }
        assertSame(failure, thrown)
        assertArrayEquals(arrayOf(cleanup), thrown.suppressed)
    }
}
