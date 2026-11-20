package dev.lancast.control

import android.os.Handler
import android.os.Looper
import org.json.JSONObject
import java.io.Closeable
import java.util.UUID
import java.util.concurrent.Executors
import java.util.concurrent.Semaphore
import java.util.concurrent.atomic.AtomicBoolean

internal object NativeCore {
    init { System.loadLibrary("lancast_core") }
    external fun create(): Long
    external fun command(handle: Long, json: String): Int
    external fun shareFile(handle: Long, fd: Int, json: String): Int
    external fun poll(handle: Long): String?
    external fun waitPoll(handle: Long, timeoutMs: Int): String?
    external fun shutdown(handle: Long)
    external fun destroy(handle: Long)
    external fun writeTs(handle: Long, bytes: java.nio.ByteBuffer): Int
}

/** One native waiter and at most one UI batch. Closing wakes both waits before destruction. */
class ControlSession(private val onEvent: (JSONObject) -> Unit) : Closeable {
    private val handle = NativeCore.create().also { check(it != 0L) { "无法启动控制核心" } }
    private val closed = AtomicBoolean(false)
    private val failed = AtomicBoolean(false)
    private val main = Handler(Looper.getMainLooper())
    private val worker = Executors.newSingleThreadExecutor()
    private val delivery = Semaphore(1)
    init {
        worker.execute {
            try {
                while (!closed.get() && !failed.get()) {
                    delivery.acquire()
                    if (closed.get() || failed.get()) break
                    val first = NativeCore.waitPoll(handle, 1000)
                    if (first == null) { delivery.release(); continue }
                    val batch = ArrayList<JSONObject>(32)
                    batch.add(JSONObject(first))
                    for (n in 1 until 32) {
                        val raw = NativeCore.poll(handle) ?: break
                        batch.add(JSONObject(raw))
                    }
                    if (!main.post {
                        try { for (event in batch) { if (closed.get() || failed.get()) break; onEvent(event) } }
                        catch (_: Exception) { fail("HOST_EVENT_HANDLER_FAILED") }
                        finally { delivery.release() }
                    }) { delivery.release(); break }
                }
            } finally { NativeCore.destroy(handle) }
        }
    }
    private fun fail(code: String = "CONTROL_COMMAND_REJECTED") {
        if (!failed.compareAndSet(false, true) || closed.get()) return
        NativeCore.shutdown(handle)
        delivery.release()
        worker.shutdown()
        // One reserved error bypasses the normal UI batch. A late media signal must
        // not throw through an Android callback after the native engine has stopped.
        main.post {
            if (!closed.get()) runCatching {
                onEvent(JSONObject().put("type", "error").put("body",
                    JSONObject().put("code", code).put("terminal", true)))
            }
        }
    }
    fun command(op: String, body: JSONObject = JSONObject()): Boolean {
        if (closed.get() || failed.get()) return false
        body.put("op", op)
        val accepted = NativeCore.command(handle, body.toString()) == 0
        if (!accepted) fail()
        return accepted
    }
    /** Native duplicates a borrowed descriptor; use closes it on every rejection/exception path. */
    fun shareFile(file: android.os.ParcelFileDescriptor, body: JSONObject): Boolean = file.use {
        if (closed.get() || failed.get()) return false
        val accepted = NativeCore.shareFile(handle, it.fd, body.put("op", "file.share").toString()) == 0
        if (!accepted) fail()
        accepted
    }
    fun send(type: String, sessionId: String?, body: JSONObject = JSONObject()): String {
        val id = UUID.randomUUID().toString()
        val message = JSONObject().put("version", 1).put("id", id)
            .put("type", type).put("sessionId", sessionId ?: JSONObject.NULL).put("body", body)
        command("send", JSONObject().put("message", message))
        return id
    }
    /** Synchronous borrow only: the caller must not retain or mutate the mux buffer. */
    fun writeTs(bytes: java.nio.ByteBuffer): Int {
        if (closed.get() || failed.get()) return -1
        if (!bytes.isDirect || bytes.position() != 0 || bytes.limit() != bytes.capacity()) return -2
        return NativeCore.writeTs(handle, bytes)
    }
    override fun close() {
        if (!closed.compareAndSet(false, true)) return
        NativeCore.shutdown(handle)
        delivery.release()
        main.removeCallbacksAndMessages(null)
        worker.shutdown()
    }
}

object SystemGuide {
    const val TEXT = "电视不能安装 App：已有 DLNA 可尝试 MP4 视频；通过合成画面与声音测试后，可尝试兼容直播。已有 Miracast 可用 Windows Win+K 或手机厂商的无线显示；已有 AirPlay 可用 Apple 控制中心的屏幕镜像。无共同协议时请外接允许安装 LanCast 的 Android HDMI 盒子。Apple TV 使用 tvOS，不能安装 Android APK。系统投屏由系统管理，不会产生 LanCast 连接统计。"
}
