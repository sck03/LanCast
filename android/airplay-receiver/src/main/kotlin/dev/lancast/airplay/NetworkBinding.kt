package dev.lancast.airplay

import android.content.Context
import android.net.*
import android.os.Handler
import java.net.Inet4Address

internal data class NetworkBinding(val network: Network, val address: String)
internal class NetworkWatcher(context: Context, private val handler: Handler, private val changed: () -> Unit) : AutoCloseable {
    private val manager = context.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
    private val notify = Runnable(changed)
    private var closed = false
    private val callback = object : ConnectivityManager.NetworkCallback() {
        override fun onAvailable(network: Network) = schedule()
        override fun onLost(network: Network) = schedule()
        override fun onLinkPropertiesChanged(network: Network, linkProperties: LinkProperties) = schedule()
        override fun onCapabilitiesChanged(network: Network, networkCapabilities: NetworkCapabilities) = schedule()
    }
    init { manager.registerNetworkCallback(NetworkRequest.Builder().addCapability(NetworkCapabilities.NET_CAPABILITY_NOT_VPN).build(), callback) }
    private fun schedule() { handler.post { if (!closed) { handler.removeCallbacks(notify); handler.postDelayed(notify, 400) } } }
    @Suppress("DEPRECATION") fun current(): NetworkBinding? = manager.allNetworks.mapNotNull { network ->
        val cap = manager.getNetworkCapabilities(network) ?: return@mapNotNull null
        if (cap.hasTransport(NetworkCapabilities.TRANSPORT_VPN) || !(cap.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) || cap.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET))) return@mapNotNull null
        val ip = manager.getLinkProperties(network)?.linkAddresses?.map { it.address }?.filterIsInstance<Inet4Address>()?.firstOrNull { it.isSiteLocalAddress || it.isLinkLocalAddress }
        ip?.hostAddress?.let { NetworkBinding(network, it) }
    }.sortedBy { it.address }.firstOrNull()
    override fun close() { closed = true; handler.removeCallbacks(notify); runCatching { manager.unregisterNetworkCallback(callback) } }
}
