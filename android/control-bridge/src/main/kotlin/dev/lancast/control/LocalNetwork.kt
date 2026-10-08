package dev.lancast.control

import android.content.Context
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.os.Build
import java.net.Inet4Address
import java.net.NetworkInterface
import java.util.Collections

private data class LocalInterface(val address: String, val prefix: Int, val name: String)
private fun interfaces(): List<LocalInterface> = runCatching {
    Collections.list(NetworkInterface.getNetworkInterfaces()).filter {
        it.isUp && !it.isLoopback && !it.isPointToPoint &&
            listOf("rmnet", "ccmni", "pdp", "tun").none { prefix -> it.name.startsWith(prefix) }
    }
        .flatMap { network -> network.interfaceAddresses.mapNotNull { item ->
            val ip = (item.address as? Inet4Address)?.hostAddress
            if (ip != null && isLanIpv4(ip)) LocalInterface(ip, item.networkPrefixLength.toInt(), network.name) else null
        } }.sortedWith(compareBy<LocalInterface> { !(it.name.startsWith("wlan") || it.name.startsWith("eth")) }.thenBy { it.name }.thenBy { it.address })
}.getOrDefault(emptyList())

fun localAddresses(context: Context? = null): List<String> {
    val all = interfaces().map { it.address }.distinct()
    if (context == null || Build.VERSION.SDK_INT < 23) return all
    val manager = context.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
    val preferred = runCatching {
        val active = manager.activeNetwork ?: return@runCatching emptyList<String>()
        val caps = manager.getNetworkCapabilities(active) ?: return@runCatching emptyList<String>()
        if (!caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) && !caps.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET)) return@runCatching emptyList<String>()
        manager.getLinkProperties(active)?.linkAddresses?.mapNotNull { (it.address as? Inet4Address)?.hostAddress } ?: emptyList()
    }.getOrDefault(emptyList())
    return (preferred.filter { it in all } + all).distinct()
}

/** Local interface metadata only: no DNS or packets, safe on the UI thread. */
fun localAddressFor(context: Context, receiverIp: String, manual: String? = null): String {
    val addresses = localAddresses(context)
    if (manual != null) return manual.takeIf { it in addresses } ?: error("所选网络已断开，请重新自动选择")
    check(isLanIpv4(receiverIp)) { "请选择有效的局域网接收端" }
    val candidates = interfaces().filter { sameSubnet(it.address, receiverIp, it.prefix) }
        .sortedWith(compareByDescending<LocalInterface> { it.prefix }.thenBy { addresses.indexOf(it.address) })
    return candidates.firstOrNull()?.address ?: addresses.firstOrNull() ?: error("请先连接与电视相同的 Wi-Fi 或网线")
}
