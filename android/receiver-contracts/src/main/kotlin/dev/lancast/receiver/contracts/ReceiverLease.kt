package dev.lancast.receiver.contracts

enum class Source { LANCAST, AIRPLAY }
data class Lease(val source: Source, val generation: Long, val session: String)

/** One shared authority for pending confirmations and active playback. Stale releases are harmless. */
class ReceiverLease {
    private var serial = 0L
    private var current: Lease? = null
    @Synchronized fun acquire(source: Source, session: String): Lease? {
        if (current != null) return null
        return Lease(source, ++serial, session).also { current = it }
    }
    @Synchronized fun owns(lease: Lease?): Boolean = lease != null && lease == current
    @Synchronized fun release(lease: Lease?): Boolean {
        if (!owns(lease)) return false
        current = null
        return true
    }
    @Synchronized fun snapshot(): Lease? = current
}

/** Process-local authority; never persists an enabled service or an old authorization. */
object ReceiverOwnership { val leases = ReceiverLease() }

enum class ServicePhase { DISABLED, STARTING, LISTENING, NETWORK_UNAVAILABLE, STOPPING }
class EnableCycle {
    var generation = 0L; private set
    var phase = ServicePhase.DISABLED; private set
    fun enable(): Long? {
        if (phase != ServicePhase.DISABLED) return null
        generation++; phase = ServicePhase.STARTING
        return generation
    }
    fun valid(token: Long) = token == generation && phase != ServicePhase.DISABLED && phase != ServicePhase.STOPPING
    fun listening(token: Long): Boolean { if (!valid(token)) return false; phase = ServicePhase.LISTENING; return true }
    fun networkLost(token: Long): Boolean { if (!valid(token)) return false; phase = ServicePhase.NETWORK_UNAVAILABLE; return true }
    fun stop(): Long { generation++; phase = ServicePhase.STOPPING; return generation }
    fun stopped(token: Long) { if (token == generation && phase == ServicePhase.STOPPING) phase = ServicePhase.DISABLED }
}
