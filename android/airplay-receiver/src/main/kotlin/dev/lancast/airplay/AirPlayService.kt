package dev.lancast.airplay

import android.app.*
import android.content.*
import android.content.pm.ServiceInfo
import android.net.wifi.WifiManager
import android.os.*
import android.view.Surface
import dev.lancast.receiver.contracts.*
import org.json.JSONObject
import java.util.concurrent.Executors

data class AirPlayRequest(val session: Long, val name: String, val peer: String)
data class AirPlayState(val enabled: Boolean = false, val message: String = "需要苹果系统投屏时开启", val name: String = "LanCast TV", val pin: String = "", val request: AirPlayRequest? = null, val playing: Boolean = false, val pairing: String = "", val videoWidth: Int = 0, val videoHeight: Int = 0)

/** One explicitly enabled cycle. Neither process recreation nor network events may enable it. */
class AirPlayService : Service() {
    inner class LocalBinder : Binder() { val service get() = this@AirPlayService }
    private val main = Handler(Looper.getMainLooper())
    private val worker = Executors.newSingleThreadExecutor { Thread(it, "airplay-lifecycle") }
    private val cycle = EnableCycle()
    private val observers = mutableSetOf<(AirPlayState) -> Unit>()
    private var state = AirPlayState()
    private var watcher: NetworkWatcher? = null
    private var network: NetworkBinding? = null
    private var discovery: AirPlayDiscovery? = null
    private var multicast: WifiManager.MulticastLock? = null
    private var surface: Surface? = null
    private var lease: Lease? = null
    @Volatile private var engine: NativeEngine? = null
    @Volatile private var media: MediaPipeline? = null
    @Volatile private var mediaSession = 0L
    @Volatile private var epoch = 0L
    private var destroyed = false
    private var cleaned = true
    private var reconfiguring = false
    private var retirement: MutableList<(Boolean) -> Unit>? = null
    override fun onBind(intent: Intent): IBinder = LocalBinder()
    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ENABLE -> enable(intent.getStringExtra("name").orEmpty())
            DISABLE -> disable()
            else -> if (!state.enabled) stopSelf()
        }
        return START_NOT_STICKY
    }
    fun observe(observer: (AirPlayState) -> Unit) { observers += observer; observer(state) }
    fun unobserve(observer: (AirPlayState) -> Unit) { observers -= observer }
    private fun update(value: AirPlayState) {
        state = value; observers.toList().forEach { it(value) }
        if (value.enabled) (getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager).notify(NOTIFICATION, notification())
    }
    private fun enable(requestedName: String) {
        if (destroyed) return
        if (!cleaned && !state.enabled) { update(state.copy(message = "正在释放接收资源，请稍后开启")); return }
        val token = cycle.enable() ?: return
        val name = requestedName.trim().ifEmpty { "LanCast TV" }
        if (name.toByteArray().size > 50 || name.any { it.isISOControl() }) { val stop = cycle.stop(); cycle.stopped(stop); update(state.copy(message = "名称请保持在 50 字节以内")); return }
        cleaned = false
        update(AirPlayState(true, "正在启动苹果投屏…", name, AirPlayIdentity.pin()))
        try {
            if (Build.VERSION.SDK_INT >= 29) startForeground(NOTIFICATION, notification(), ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE)
            else startForeground(NOTIFICATION, notification())
            val wifi = applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
            multicast = wifi.createMulticastLock("lancast-airplay").apply { setReferenceCounted(false); acquire() }
            watcher = NetworkWatcher(this, main) { if (cycle.valid(token)) checkNetwork(token) }
            checkNetwork(token)
        } catch (e: Exception) { disable(e.message ?: "AirPlay 服务无法启动") }
    }
    private fun checkNetwork(token: Long) {
        if (!cycle.valid(token) || reconfiguring) return
        val binding = watcher?.current()
        if (binding == network && (engine != null || binding == null && cycle.phase == ServicePhase.NETWORK_UNAVAILABLE)) return
        reconfiguring = true
        retireEngine { success ->
            reconfiguring = false
            if (!cycle.valid(token)) return@retireEngine
            if (!success) { disable("设备发现未能完整停止，请稍后重新开启"); return@retireEngine }
            val latest = watcher?.current(); network = latest
            if (latest == null) { cycle.networkLost(token); update(state.copy(message = "网络不可用，连接局域网后继续等待")); return@retireEngine }
            startEngine(token, latest)
        }
    }
    private fun startEngine(token: Long, binding: NetworkBinding) {
        val current = ++epoch
        val name = state.name; val pin = state.pin
        update(state.copy(message = "正在发布苹果接收设备…"))
        val listener = object : NativeEngine.Listener {
            override fun event(session: Long, type: Int, text: String) { main.post {
                if (current != epoch || !cycle.valid(token)) return@post
                when (type) {
                    1 -> try {
                        discovery = AirPlayDiscovery(this@AirPlayService, main, { disable(it) }) {
                            if (current == epoch && cycle.listening(token)) update(state.copy(message = "等待苹果设备，在控制中心选择“屏幕镜像”"))
                        }.also { it.publish(JSONObject(text), binding) }
                    } catch (e: Exception) { disable(e.message ?: "设备发布失败") }
                    2 -> incoming(session, JSONObject(text))
                    3 -> if (mediaSession == session || state.request?.session == session) finishSession(when (text) {
                        "TIMING_UNAVAILABLE" -> "无法与苹果设备同步时钟，请重新连接"
                        "MEDIA_BACKPRESSURE" -> "电视播放处理不及时，请降低发送分辨率后重试"
                        "INVALID_VIDEO_CONFIG", "HEVC_UNSUPPORTED" -> "电视无法播放此次视频格式"
                        "VIDEO_TRANSPORT_FAILED", "AUDIO_TRANSPORT_FAILED" -> "镜像数据接收失败，请重新连接"
                        else -> "投屏已结束，继续等待连接"
                    })
                    4 -> disable("AirPlay 引擎启动或网络失败，请关闭后重试")
                    5 -> if (mediaSession == session) text.toFloatOrNull()?.let { media?.setVolume(it) }
                }
            } }
            override fun videoConfig(session: Long, width: Int, height: Int, sps: ByteArray, pps: ByteArray): Boolean {
                if (current != epoch || mediaSession != session) return false
                main.post { if (current == epoch && mediaSession == session) update(state.copy(videoWidth = width, videoHeight = height)) }
                return media?.videoConfig(width, height, sps, pps) == true
            }
            override fun video(session: Long, pts: Long, key: Boolean, data: ByteArray) = current == epoch && mediaSession == session && media?.video(pts, key, data) == true
            override fun audioConfig(session: Long, codec: Int, rate: Int, channels: Int, spf: Int) = current == epoch && mediaSession == session && media?.audioConfig(codec, rate, channels, spf) == true
            override fun audio(session: Long, pts: Long, data: ByteArray) = current == epoch && mediaSession == session && media?.audio(pts, data) == true
        }
        val opened = try { NativeEngine(listener) } catch (e: Throwable) { disable(e.message ?: "AirPlay 模块加载失败"); return }
        engine = opened
        worker.execute {
            try {
                MediaPipeline.checkCapabilities()
                if (current == epoch) opened.start(binding.address, name, AirPlayIdentity.seed(this), pin, java.io.File(noBackupFilesDir, "airplay-peers-v1.json").absolutePath)
            } catch (e: Throwable) {
                opened.close()
                main.post { if (current == epoch && cycle.valid(token)) disable(e.message ?: "AirPlay 模块加载失败") }
            }
        }
    }
    private fun incoming(session: Long, body: JSONObject) {
        if (lease != null) { engine?.approve(session, false); return }
        val acquired = ReceiverOwnership.leases.acquire(Source.AIRPLAY, "$epoch:$session")
        if (acquired == null) { engine?.approve(session, false); return }
        lease = acquired
        update(state.copy(message = "有苹果设备请求连接，请回到接收页面确认", pairing = body.optString("pairing"), request = AirPlayRequest(session, body.optString("name"), body.optString("peer"))))
        val expectedEpoch = epoch
        main.postDelayed({ if (epoch == expectedEpoch && state.request?.session == session) { engine?.approve(session, false); finishSession("确认超时，请在苹果设备重新选择接收端") } }, 15_000)
    }
    fun approve(session: Long, accept: Boolean) {
        if (state.request?.session != session) return
        val display = surface
        if (!accept || display == null || !display.isValid) { engine?.approve(session, false); finishSession(if (accept) "请保持接收页面打开后重新连接" else "已拒绝连接，继续等待"); return }
        mediaSession = session
        val expectedEpoch = epoch
        media = MediaPipeline(this, display, {
            main.post { if (epoch == expectedEpoch && mediaSession == session) update(state.copy(playing = true, message = "正在接收苹果屏幕")) }
        }, { reason -> main.post { if (epoch == expectedEpoch && mediaSession == session) { engine?.stopSession(session); finishSession(reason) } } })
        update(state.copy(request = null, message = "正在建立苹果镜像连接…"))
        engine?.approve(session, true)
        main.postDelayed({ if (epoch == expectedEpoch && mediaSession == session && !state.playing) { engine?.stopSession(session); finishSession("未收到可播放的画面，请重新连接") } }, 10_000)
    }
    fun attachDisplay(value: Surface?) {
        surface = value
        if (value == null && mediaSession != 0L) { engine?.stopSession(mediaSession); finishSession("接收页面已离开，继续后台等待连接") }
    }
    fun stopCurrent() { state.request?.let { engine?.approve(it.session, false) }; if (mediaSession != 0L) engine?.stopSession(mediaSession); finishSession("投屏已停止，继续等待连接") }
    private fun finishSession(message: String) {
        mediaSession = 0
        val player = media; media = null
        val held = lease; lease = null
        update(state.copy(request = null, playing = false, message = message, videoWidth = 0, videoHeight = 0))
        worker.execute { player?.close(); ReceiverOwnership.leases.release(held) }
    }
    private fun retireEngine(done: (Boolean) -> Unit) {
        epoch++
        retirement?.let { it += done; return }
        val callbacks = mutableListOf(done); retirement = callbacks
        val publisher = discovery; discovery = null
        val previous = engine; engine = null
        finishSession("正在切换接收网络…")
        var nativeDone = false
        var discoveryDone: Boolean? = null
        var completed = false
        fun complete() {
            if (completed || destroyed || !nativeDone || discoveryDone == null) return
            completed = true
            retirement = null
            callbacks.toList().forEach { it(discoveryDone == true) }
        }
        if (publisher == null) discoveryDone = true else publisher.close { discoveryDone = it; complete() }
        worker.execute { previous?.close(); main.post { nativeDone = true; complete() } }
    }
    fun disable(reason: String = "AirPlay 已关闭") {
        val stopped = cycle.stop()
        watcher?.close(); watcher = null; network = null
        update(state.copy(enabled = false, pin = "", request = null, playing = false, message = reason))
        retireEngine { _ ->
            multicast?.let { if (it.isHeld) it.release() }; multicast = null
            cycle.stopped(stopped); cleaned = true
            if (Build.VERSION.SDK_INT >= 24) stopForeground(STOP_FOREGROUND_REMOVE) else { @Suppress("DEPRECATION") stopForeground(true) }
            update(AirPlayState(message = reason, name = state.name)); stopSelf()
        }
    }
    private fun notification(): Notification {
        val manager = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        if (Build.VERSION.SDK_INT >= 26) manager.createNotificationChannel(NotificationChannel(CHANNEL, "苹果投屏接收", NotificationManager.IMPORTANCE_LOW))
        val launch = packageManager.getLaunchIntentForPackage(packageName)!!
        val open = PendingIntent.getActivity(this, 0, launch, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
        val stop = PendingIntent.getService(this, 1, Intent(this, AirPlayService::class.java).setAction(DISABLE), PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
        val builder = if (Build.VERSION.SDK_INT >= 26) Notification.Builder(this, CHANNEL) else { @Suppress("DEPRECATION") Notification.Builder(this) }
        return builder.setSmallIcon(android.R.drawable.ic_media_play).setContentTitle("${state.name} · 苹果投屏")
            .setContentText(state.message).setContentIntent(open).setOngoing(true)
            .addAction(Notification.Action.Builder(null, "关闭接收", stop).build()).build()
    }
    override fun onTaskRemoved(rootIntent: Intent?) { disable(); super.onTaskRemoved(rootIntent) }
    override fun onDestroy() {
        destroyed = true; epoch++; cycle.stop()
        watcher?.close(); discovery?.close(); main.removeCallbacksAndMessages(null)
        val previous = engine; engine = null; val player = media; media = null
        val held = lease; lease = null
        worker.execute { previous?.close(); player?.close(); ReceiverOwnership.leases.release(held); multicast?.let { if (it.isHeld) it.release() } }
        worker.shutdown(); observers.clear(); super.onDestroy()
    }
    companion object {
        const val ENABLE = "dev.lancast.airplay.ENABLE"
        const val DISABLE = "dev.lancast.airplay.DISABLE"
        private const val CHANNEL = "airplay-receiver"
        private const val NOTIFICATION = 7301
        fun enable(context: Context, name: String) {
            val intent = Intent(context, AirPlayService::class.java).setAction(ENABLE).putExtra("name", name)
            if (Build.VERSION.SDK_INT >= 26) context.startForegroundService(intent) else context.startService(intent)
        }
    }
}
