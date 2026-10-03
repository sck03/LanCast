package dev.lancast.control

import android.os.Handler
import android.os.Looper
import org.json.JSONObject
import java.io.Closeable
import java.net.Inet4Address
import java.net.NetworkInterface
import java.util.Collections
import java.util.UUID
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

internal object NativeCore {
    init { System.loadLibrary("lancast_core") }
    external fun create(): Long
    external fun command(handle: Long, json: String): Int
    external fun poll(handle: Long): String?
    external fun destroy(handle: Long)
    external fun writeTs(handle: Long, bytes: ByteArray): Int
}

/** The UI receives immutable JSON events; media frames never cross JNI here. */
class ControlSession(private val onEvent: (JSONObject) -> Unit) : Closeable {
    private val handle = NativeCore.create().also { check(it != 0L) { "无法启动控制核心" } }
    private val closed = AtomicBoolean(false)
    private val main = Handler(Looper.getMainLooper())
    private val worker = Executors.newSingleThreadScheduledExecutor()
    init {
        worker.scheduleWithFixedDelay({
            if (!closed.get()) {
                repeat(32) {
                    val raw = NativeCore.poll(handle) ?: return@scheduleWithFixedDelay
                    val event = JSONObject(raw)
                    main.post { if (!closed.get()) onEvent(event) }
                }
            }
        }, 0, 30, TimeUnit.MILLISECONDS)
    }
    fun command(op: String, body: JSONObject = JSONObject()) {
        if (closed.get()) return
        body.put("op", op)
        check(NativeCore.command(handle, body.toString()) == 0) { "控制队列已满或请求无效" }
    }
    fun send(type: String, sessionId: String?, body: JSONObject = JSONObject()) {
        val message = JSONObject().put("version", 1).put("id", UUID.randomUUID().toString())
            .put("type", type).put("sessionId", sessionId ?: JSONObject.NULL).put("body", body)
        command("send", JSONObject().put("message", message))
    }
    fun writeTs(bytes: ByteArray): Int = if (closed.get()) -1 else NativeCore.writeTs(handle, bytes)
    override fun close() {
        if (!closed.compareAndSet(false, true)) return
        worker.execute { NativeCore.destroy(handle) }
        worker.shutdown()
    }
}

fun localAddresses(): List<String> = Collections.list(NetworkInterface.getNetworkInterfaces())
    .filter { it.isUp && !it.isLoopback }.flatMap { Collections.list(it.inetAddresses) }
    .filterIsInstance<Inet4Address>().filter { it.isSiteLocalAddress }.map { it.hostAddress!! }

object SystemGuide {
    const val TEXT = "电视不能安装 App：已有 DLNA 可播放 MP4 视频，但不能分享桌面。已有 Miracast 可用 Windows Win+K 或手机厂商的无线显示；已有 AirPlay 可用 Apple 控制中心的屏幕镜像。无共同协议时请外接允许安装 LanCast 的 Android HDMI 盒子。Apple TV 使用 tvOS，不能安装 Android APK。系统投屏由系统管理，不会产生 LanCast 连接统计。"
}
