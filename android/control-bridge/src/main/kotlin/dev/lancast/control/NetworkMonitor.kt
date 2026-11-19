package dev.lancast.control

import android.content.Context
import android.net.ConnectivityManager
import android.net.LinkProperties
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import android.os.Handler
import android.os.Looper
import java.io.Closeable

/** Reconcile only after a network transition; no periodic interface polling. */
class NetworkMonitor(context: Context, changed: () -> Unit) : Closeable {
    private val manager = context.applicationContext.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
    private val handler = Handler(Looper.getMainLooper())
    private val notify = Runnable(changed)
    private var closed = false
    private fun schedule() { handler.post { if (!closed) { handler.removeCallbacks(notify); handler.postDelayed(notify, 400) } } }
    private val callback = object : ConnectivityManager.NetworkCallback() {
        override fun onAvailable(network: Network) = schedule()
        override fun onLost(network: Network) = schedule()
        override fun onLinkPropertiesChanged(network: Network, linkProperties: LinkProperties) = schedule()
    }
    init { manager.registerNetworkCallback(NetworkRequest.Builder().addCapability(NetworkCapabilities.NET_CAPABILITY_NOT_VPN).build(), callback) }
    override fun close() {
        closed = true; handler.removeCallbacksAndMessages(null)
        runCatching { manager.unregisterNetworkCallback(callback) }
    }
}
