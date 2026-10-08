package dev.lancast.airplay

import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.os.Build
import android.os.Handler
import org.json.JSONObject

/** A single publisher owns both records. A stale registration immediately unregisters itself. */
internal class AirPlayDiscovery(context: Context, private val handler: Handler, private val failed: (String) -> Unit, private val ready: (String) -> Unit) : AutoCloseable {
    private val manager = context.getSystemService(Context.NSD_SERVICE) as NsdManager
    private val listeners = mutableListOf<NsdManager.RegistrationListener>()
    private val pending = mutableSetOf<NsdManager.RegistrationListener>()
    private val closeCallbacks = mutableListOf<(Boolean) -> Unit>()
    private var closeResult: Boolean? = null
    private var withdrawalFailed = false
    private var closed = false
    private var registered = 0
    private var name = ""
    private val withdrawalTimeout = Runnable { finishWithdrawal(false) }
    private fun finishWithdrawal(success: Boolean) {
        if (closeResult != null) return
        closeResult = success
        handler.removeCallbacks(withdrawalTimeout)
        closeCallbacks.toList().forEach { it(success) }; closeCallbacks.clear()
    }
    private fun withdrawn(listener: NsdManager.RegistrationListener, success: Boolean) {
        pending.remove(listener); withdrawalFailed = withdrawalFailed || !success
        if (closed && pending.isEmpty()) finishWithdrawal(!withdrawalFailed)
    }
    fun publish(info: JSONObject, binding: NetworkBinding) {
        check(listeners.isEmpty() && !closed)
        name = info.getString("name")
        val device = info.getString("deviceId")
        val features = info.getString("features")
        val publicKey = info.getString("publicKey")
        fun register(type: String, instance: String, attributes: Map<String, String>) {
            val listener = object : NsdManager.RegistrationListener {
                override fun onServiceRegistered(service: NsdServiceInfo) { handler.post {
                    if (closed) { runCatching { manager.unregisterService(this) }; return@post }
                    // Do not publish two conflicting identities after an automatic NSD rename.
                    if (service.serviceName != instance) { failed("接收名称冲突，请修改 AirPlay 名称后重试"); return@post }
                    registered++; if (registered == 2) ready(name)
                } }
                override fun onRegistrationFailed(service: NsdServiceInfo, code: Int) { handler.post { withdrawn(this, true); if (!closed) failed("局域网设备发布失败（$code）") } }
                override fun onServiceUnregistered(service: NsdServiceInfo) { handler.post { withdrawn(this, true) } }
                override fun onUnregistrationFailed(service: NsdServiceInfo, code: Int) { handler.post { withdrawn(this, false) } }
            }
            val service = NsdServiceInfo().apply {
                serviceType = type; serviceName = instance; port = info.getInt("port")
                if (Build.VERSION.SDK_INT >= 33) network = binding.network
                for ((key, value) in attributes) setAttribute(key, value)
            }
            listeners += listener
            pending += listener
            manager.registerService(service, NsdManager.PROTOCOL_DNS_SD, listener)
        }
        register("_airplay._tcp.", name, mapOf("deviceid" to device, "features" to features, "model" to "LanCast", "srcvers" to "770.8.1", "flags" to "0x4", "vv" to "2", "pk" to publicKey, "pi" to device))
        register("_raop._tcp.", device.replace(":", "") + "@" + name, mapOf("cn" to "0,2,4", "ch" to "2", "et" to "0,3,5", "sr" to "44100", "ss" to "16", "tp" to "UDP", "txtvers" to "1", "vn" to "65537", "vs" to "770.8.1", "am" to "LanCast", "ft" to features, "pk" to publicKey))
    }
    fun close(completed: (Boolean) -> Unit) {
        closeResult?.let { completed(it); return }
        closeCallbacks += completed
        if (closed) return
        closed = true
        if (pending.isEmpty()) { finishWithdrawal(true); return }
        listeners.forEach { runCatching { manager.unregisterService(it) } }
        // Pending registration callbacks also unregister themselves. Never restart a new
        // publisher while withdrawal is unresolved; a timeout is reported to the service.
        handler.postDelayed(withdrawalTimeout, 3_000)
    }
    override fun close() = close { }
}
