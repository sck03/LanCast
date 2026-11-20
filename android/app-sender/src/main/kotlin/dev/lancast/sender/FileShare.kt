package dev.lancast.sender

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.CancellationSignal
import android.os.Handler
import android.os.Looper
import android.os.ParcelFileDescriptor
import org.json.JSONObject
import java.util.UUID
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit

/** File service owns provider I/O; only the main thread changes request/session state. */
internal class FileShare(private val changed: (String) -> Unit) {
    enum class Phase { IDLE, PREPARING, ESTABLISHING, LOADING, READY }
    private data class Request(val id: String, val uri: Uri, val body: JSONObject)
    private val main = Handler(Looper.getMainLooper())
    // One stuck document provider cannot create unbounded workers or queued descriptors.
    private val worker = ThreadPoolExecutor(1, 1, 10, TimeUnit.SECONDS, ArrayBlockingQueue(1)).apply { allowCoreThreadTimeOut(true) }
    private var request: Request? = null
    private var awaitingInitialReady = false
    private var cancellation: CancellationSignal? = null
    private var stopService: (() -> Unit)? = null
    var phase = Phase.IDLE
        private set
    val active get() = phase != Phase.IDLE
    val ready get() = phase == Phase.READY
    fun prepare(context: Context, uri: Uri, local: String, receiver: String, encrypted: Boolean) {
        check(!active)
        val id = UUID.randomUUID().toString()
        request = Request(id, uri, JSONObject().put("address", "$local:0").put("allowedIp", receiver).put("encrypted", encrypted))
        phase = Phase.PREPARING; awaitingInitialReady = true
        try { context.startForegroundService(Intent(context, FileShareService::class.java).putExtra("requestId", id)) }
        catch (error: Exception) { close(); throw error }
    }
    fun open(context: Context, id: String, stopped: () -> Unit, submit: (ParcelFileDescriptor, JSONObject) -> Boolean, failed: (String) -> Unit): Boolean {
        val current = request?.takeIf { it.id == id && phase == Phase.PREPARING && stopService == null } ?: return false
        stopService = stopped
        val signal = CancellationSignal(); cancellation = signal
        val timeout = Runnable { if (request === current && phase == Phase.PREPARING) { close(); failed("文件读取超时，请重新选择文件") } }
        main.postDelayed(timeout, 30_000)
        try {
            worker.execute {
                val result = runCatching { context.contentResolver.openFileDescriptor(current.uri, "r", signal) ?: error("无法打开文件") }
                main.post {
                    main.removeCallbacks(timeout)
                    val descriptor = result.getOrNull()
                    if (request !== current || signal.isCanceled) { descriptor?.close(); return@post }
                    cancellation = null
                    try {
                        checkNotNull(descriptor) { result.exceptionOrNull()?.message ?: "无法打开文件" }
                        descriptor.use { check(submit(it, current.body)) { "控制服务已停止" } }
                        phase = Phase.ESTABLISHING; changed("正在建立文件会话…")
                        main.postDelayed({
                            if (request === current && awaitingInitialReady) { close(); failed("电视准备播放超时，请重试") }
                        }, 20_000)
                    } catch (error: Exception) { close(); failed(error.message ?: "文件读取失败") }
                }
            }
        } catch (error: Exception) { main.removeCallbacks(timeout); close(); failed("文件读取器正忙，请稍后重试") }
        return true
    }
    fun loading() { if (active) phase = Phase.LOADING }
    fun state(ready: Boolean) {
        if (phase == Phase.LOADING || phase == Phase.READY) {
            phase = if (ready) Phase.READY else Phase.LOADING
            if (ready) awaitingInitialReady = false
        }
    }
    fun owns(id: String) = request?.id == id
    fun close() {
        request = null; phase = Phase.IDLE
        cancellation?.cancel(); cancellation = null
        val stop = stopService; stopService = null; stop?.invoke()
    }
}
