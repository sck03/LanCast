package dev.lancast.control

import org.json.JSONArray
import java.util.Locale

data class PairingInput(val address: String, val fingerprint: String, val code: String)

data class CaptureTarget(val receiver: String, val dlnaId: String?, val dlnaIp: String?, val local: String)
/** A system grant belongs to one explicit target and is consumed at most once. */
class CaptureGrantGate {
    private var request: Pair<String, CaptureTarget>? = null
    val pending get() = request != null
    fun prepare(target: CaptureTarget): String = java.util.UUID.randomUUID().toString().also { request = it to target }
    fun consume(id: String, target: CaptureTarget): Boolean {
        if (request != (id to target)) return false
        request = null; return true
    }
    fun cancel(id: String? = null) { if (id == null || request?.first == id) request = null }
}

/** Public discovery data only; a hint is never permission to connect. */
data class ReceiverHint(val id: String, val name: String, val ip: String, val port: Int,
                        val fingerprint: String, val dlna: Boolean) {
    val key get() = (if (dlna) "dlna:" else "lancast:") + id
    val address get() = "$ip:$port"
    val label get() = "$name · ${if (dlna) "普通电视 (DLNA)" else "LanCast"} · $ip"
}

fun ipv4Number(value: String): Long? {
    val parts = value.split('.')
    if (parts.size != 4) return null
    var result = 0L
    for (part in parts) {
        if (part.isEmpty() || part.length > 3 || part.any { it !in '0'..'9' } || (part.length > 1 && part[0] == '0')) return null
        val n = part.toIntOrNull() ?: return null
        if (n !in 0..255) return null
        result = (result shl 8) or n.toLong()
    }
    return result
}
fun isLanIpv4(value: String): Boolean {
    val n = ipv4Number(value) ?: return false
    return n ushr 24 == 10L || n ushr 20 == 0xac1L || n ushr 16 == 0xc0a8L || n ushr 16 == 0xa9feL
}
fun sameSubnet(local: String, remote: String, prefix: Int): Boolean {
    if (prefix !in 1..32) return false
    val a = ipv4Number(local) ?: return false
    val b = ipv4Number(remote) ?: return false
    val mask = (0xffffffffL shl (32 - prefix)) and 0xffffffffL
    return (a and mask) == (b and mask)
}
fun normalizedFingerprint(value: String): String? {
    val pin = value.lowercase(Locale.ROOT)
    return pin.takeIf { it.length == 64 && it.all { c -> c in '0'..'9' || c in 'a'..'f' } }
}
class DeviceCatalog {
    private val entries = linkedMapOf<String, ReceiverHint>()
    val devices get() = entries.values.sortedWith(compareBy<ReceiverHint> { it.dlna }.thenBy { it.key })
    fun clear() = entries.clear()
    fun update(records: JSONArray, dlna: Boolean) {
        entries.entries.removeAll { it.value.dlna == dlna }
        for (n in 0 until minOf(records.length(), 128)) {
            val record = records.optJSONObject(n) ?: continue
            val id = (record.opt("id") as? String)?.takeIf { it.isNotEmpty() && it.length <= 512 } ?: continue
            val name = (record.opt("name") as? String ?: "TV").removeSuffix("._lancast._tcp.local.")
                .filterNot { it.isISOControl() }.take(100)
            val addresses = record.optJSONArray("addresses") ?: JSONArray()
            val ip = if (dlna) record.opt("ip") as? String else
                (0 until minOf(addresses.length(), 32)).mapNotNull { addresses.opt(it) as? String }.firstOrNull(::isLanIpv4)
            if (ip == null || !isLanIpv4(ip)) continue
            val rawPort = record.opt("port")
            val port = if (dlna) 0 else when (rawPort) { is Int -> rawPort; is Long -> rawPort.takeIf { it in 1..65535 }?.toInt() ?: 0; else -> 0 }
            if (!dlna && port !in 1..65535) continue
            val rawPin = record.opt("fingerprint") as? String ?: ""
            val pin = if (rawPin.length == 64) normalizedFingerprint(rawPin) ?: "" else ""
            if (!dlna && pin.isEmpty()) continue
            val hint = ReceiverHint(id, name, ip, port, pin, dlna)
            if (entries.size < 128) entries[hint.key] = hint
        }
    }
}
