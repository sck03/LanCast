package dev.lancast.control

import org.junit.Assert.*
import org.junit.Test
import org.json.JSONArray
import org.json.JSONObject

class ConnectionHintsTest {
    @Test fun captureGrantCannotCrossTargetsStopOrReplay() {
        val gate = CaptureGrantGate()
        val tv = CaptureTarget("192.168.1.8:8787", null, null, "192.168.1.3")
        val request = gate.prepare(tv)
        assertFalse(gate.consume(request, tv.copy(receiver = "192.168.1.9:8787")))
        assertTrue(gate.consume(request, tv)); assertFalse(gate.consume(request, tv))
        val old = gate.prepare(tv); gate.cancel(); assertFalse(gate.consume(old, tv))
        val newer = gate.prepare(tv); gate.cancel(old)
        assertTrue(gate.consume(newer, tv))
    }
    @Test fun addressesAndPinsAreValidated() {
        assertTrue(isLanIpv4("192.168.1.4")); assertTrue(isLanIpv4("172.16.1.4"))
        for (ip in listOf("127.0.0.1", "0.0.0.0", "8.8.8.8", "192.168.001.2", "host.local", "999.1.1.1")) assertFalse(ip, isLanIpv4(ip))
        assertTrue(sameSubnet("192.168.1.4", "192.168.1.8", 24))
        assertFalse(sameSubnet("192.168.1.4", "192.168.2.8", 24))
        assertEquals("a".repeat(64), normalizedFingerprint("A".repeat(64)))
        assertNull(normalizedFingerprint("AA:".repeat(31) + "AA"))
        assertNull(normalizedFingerprint("g".repeat(64)))
    }
    @Test fun discoveryMergesProtocolsAndDoesNotRetainOldIdentity() {
        val native = JSONObject().put("id", "tv").put("name", "客厅._lancast._tcp.local.")
            .put("addresses", JSONArray(listOf("::1", "192.168.1.8"))).put("port", 8787).put("fingerprint", "a".repeat(64))
        val dlna = JSONObject().put("id", "tv").put("name", "客厅").put("ip", "192.168.1.8")
        val catalog = DeviceCatalog()
        catalog.update(JSONArray().put(native).put(native), false)
        catalog.update(JSONArray().put(dlna), true)
        assertEquals(2, catalog.devices.size); assertEquals("192.168.1.8:8787", catalog.devices[0].address)
        native.remove("fingerprint"); catalog.update(JSONArray().put(native), false)
        assertEquals(1, catalog.devices.size); assertTrue(catalog.devices[0].dlna)
        catalog.update(JSONArray().put(JSONObject().put("id", "bad").put("port", "bad")), false)
        assertEquals(1, catalog.devices.size); assertTrue(catalog.devices[0].dlna)
    }
}
