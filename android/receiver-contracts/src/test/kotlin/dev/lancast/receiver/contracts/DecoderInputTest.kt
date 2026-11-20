package dev.lancast.receiver.contracts

import org.junit.Assert.*
import org.junit.Test

class DecoderInputTest {
    @Test fun retainsInputAndRetriesDoNotExtendDeadline() {
        val input = DecoderInput<String>(100)
        input.defer("first", 10)
        input.defer("first", 109)
        assertEquals("first", input.pending)
        assertThrows(IllegalStateException::class.java) { input.defer("first", 110) }
    }
    @Test fun acceptingStartsAFreshBudgetForNextInput() {
        val input = DecoderInput<String>(100)
        input.defer("first", 0); input.accepted()
        assertNull(input.pending)
        input.defer("second", 1000)
        assertEquals("second", input.pending)
    }
}
