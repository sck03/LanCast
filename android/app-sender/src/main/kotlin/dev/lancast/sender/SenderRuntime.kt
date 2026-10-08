package dev.lancast.sender

import android.content.Context
import android.content.Intent
import dev.lancast.control.ControlSession
import dev.lancast.control.DeviceCatalog
import dev.lancast.control.PairingInput
import dev.lancast.control.CaptureGrantGate
import dev.lancast.control.CaptureTarget
import android.net.wifi.WifiManager
import dev.lancast.media.RtcPeer
import org.json.JSONObject

/** One coordinator owns capture and control even while the Activity is backgrounded. */
object SenderRuntime {
    var observer: ((JSONObject) -> Unit)? = null
    private var core: ControlSession? = null
    private var peer: RtcPeer? = null
    private var liveCapture: DlnaCapture? = null
    private var liveUrl: String? = null
    var localAddress = ""
    private var generation = 0L
    private var sessionId: String? = null
    private var pendingSession: String? = null
    private var grant: Intent? = null
    private var context: Context? = null
    private var withAudio = false
    var probing = false
        private set
    private var profilesPath: String? = null
    private var media: JSONObject? = null
    var receiverAddress = ""
    var connected = false
        private set
    var dlnaId: String? = null
    var dlnaIp: String? = null
    private var stopService: (() -> Unit)? = null
    val catalog = DeviceCatalog()
    var selectedDevice: String? = null
    var connecting = false
        private set
    var scanning = false
        private set
    private var scanGeneration = 0L
    private var nativePending = false
    private var dlnaPending = false
    private var multicast: WifiManager.MulticastLock? = null
    private var filePending = false
    private var stopping = false
    private val captureGate = CaptureGrantGate()
    private var activeCapture: String? = null
    val busy get() = captureGate.pending || peer != null || liveCapture != null || grant != null || probing || pendingSession != null || sessionId != null || media != null || filePending || stopping
    val filePlaying get() = media != null && !stopping && liveCapture == null && peer == null
    val selectionLocked get() = connected || connecting || scanning || busy
    fun scan(context: Context, local: String) {
        check(!selectionLocked) { "请先停止分享并断开连接" }
        ensureCore(context); catalog.clear(); selectedDevice = null; dlnaId = null; dlnaIp = null; receiverAddress = ""
        scanGeneration++; val current = scanGeneration
        scanning = true; nativePending = true; dlnaPending = local.isNotEmpty()
        releaseMulticast()
        try {
        val wifi = context.applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
        multicast = wifi.createMulticastLock("lancast-scan").apply { setReferenceCounted(false); acquire() }
        command("scan", JSONObject().put("scanGeneration", current))
        if (dlnaPending) command("dlna.scan", JSONObject().put("interface", local).put("scanGeneration", current))
        } catch (error: Exception) {
            scanning = false; nativePending = false; dlnaPending = false; releaseMulticast(); throw error
        }
        android.os.Handler(android.os.Looper.getMainLooper()).postDelayed({
            if (scanning && scanGeneration == current) {
                scanning = false; nativePending = false; dlnaPending = false; releaseMulticast()
                observer?.invoke(JSONObject().put("type", "discovery.updated"))
            }
        }, 25000)
    }
    fun connect(input: PairingInput) {
        check(!selectionLocked) { "请先停止分享并断开连接" }
        receiverAddress = input.address; dlnaId = null; dlnaIp = null
        command("connect", JSONObject().put("address", input.address).put("fingerprint", input.fingerprint).put("invite", input.code).put("name", android.os.Build.MODEL))
        connecting = true
    }
    private fun releaseMulticast() { multicast?.let { if (it.isHeld) it.release() }; multicast = null }
    fun ensureCore(context: Context? = null) {
        if (context != null) profilesPath = java.io.File(context.filesDir, "receiver-profiles.json").absolutePath
        if (core == null) { core = ControlSession(::onEvent); profilesPath?.let { core!!.command("profiles.open", JSONObject().put("path", it)) } }
    }
    fun command(op: String, body: JSONObject = JSONObject()) {
        ensureCore()
        if (op == "file.share") { check(!busy && (connected || dlnaId != null)) { "请先连接电视并停止当前分享" } }
        core!!.command(op, body)
        if (op == "file.share") filePending = true
    }
    private fun captureTarget() = CaptureTarget(receiverAddress, dlnaId, dlnaIp, localAddress)
    fun prepareCapture(): String {
        check(!busy && (connected || dlnaId != null)) { "请先连接电视并停止当前分享" }
        return captureGate.prepare(captureTarget())
    }
    fun cancelCapture(id: String? = null) = captureGate.cancel(id)
    fun stopCaptureService(id: String) { if (activeCapture == id) stop() }
    fun captureFailure(id: String, message: String) {
        captureGate.cancel(id); stopCaptureService(id)
        observer?.invoke(JSONObject().put("type", "media.status").put("body", JSONObject().put("status", "无法开始分享：$message")))
    }
    fun beginCapture(context: Context, permission: Intent, audio: Boolean, requestId: String, stopService: () -> Unit) {
        check(captureGate.consume(requestId, captureTarget())) { "分享请求已取消或目标已改变，请重新授权" }
        check(connected || dlnaId != null)
        check(peer == null && liveCapture == null && grant == null) { "先停止当前分享" }
        generation++
        activeCapture = requestId
        this.context = context.applicationContext; grant = permission; withAudio = audio; this.stopService = stopService
        probing = false
        if (dlnaId != null) createLive()
        else pendingSession = core!!.send("session.start", null, JSONObject().put("mode", "mirror").put("audioRequested", audio).put("rtcRecovery", "replace-v1"))
    }
    fun beginProbe(context: Context, audio: Boolean) {
        check(dlnaId != null && liveCapture == null && peer == null && grant == null) { "先选择电视并停止当前分享" }
        ensureCore(context); this.context = context.applicationContext; withAudio = audio; probing = true; generation++; createLive()
    }
    private fun createLive() { command("live.create", JSONObject().put("address", "$localAddress:0").put("allowedIp", dlnaIp).put("deviceId", dlnaId).put("synthetic", probing).put("audio", withAudio).put("generation", generation)) }
    fun playback(action: String, positionMs: Long = 0) {
        check(media != null && !stopping && liveCapture == null && peer == null) { "播放控制仅用于当前视频文件" }
        if (dlnaId != null) command("dlna.command", JSONObject().put("deviceId", dlnaId).put("action", action).put("positionMs", positionMs))
        else core?.send("playback.command", sessionId, JSONObject().put("action", action).put("positionMs", positionMs))
    }
    private fun onEvent(event: JSONObject) {
        val body = event.optJSONObject("body") ?: JSONObject()
        if (event.optString("type").startsWith("live.") && body.has("generation") && body.optLong("generation", -1) != generation) return
        when (event.optString("type")) {
            "devices", "dlna.devices" -> {
                if (!scanning || body.optLong("scanGeneration", -1) != scanGeneration) return
                val dlna = event.optString("type") == "dlna.devices"
                catalog.update(body.optJSONArray("devices") ?: org.json.JSONArray(), dlna)
                if (dlna) dlnaPending = false else nativePending = false
                scanning = nativePending || dlnaPending
                if (!scanning) releaseMulticast()
            }
            "live.created" -> {
                if (body.optLong("generation", -1) != generation) return
                val permission = grant
                if (permission == null && !probing) return
                grant = null; liveUrl = body.getString("url")
                val active = generation
                liveCapture = DlnaCapture(context!!, { bytes -> core?.writeTs(bytes) ?: -1 }) { value ->
                    android.os.Handler(android.os.Looper.getMainLooper()).post {
                        if (generation == active) {
                            if (value == "dlna_capture_started") command("dlna.load", JSONObject().put("deviceId", dlnaId).put("url", liveUrl).put("title", "LanCast Live").put("live", true))
                            else stop() // All other DlnaCapture callbacks are terminal errors/revocation.
                            observer?.invoke(JSONObject().put("type", "media.status").put("body", JSONObject().put("status", value)))
                        }
                    }
                }.also { if (probing) it.startProbe(withAudio) else it.start(checkNotNull(permission), withAudio) }
            }
            "probe.saved" -> stop()
            "live.failed" -> { stop(); observer?.invoke(JSONObject().put("type", "error").put("body", body)) }
            "error" -> { connecting = false; filePending = false; if (captureGate.pending || peer != null || liveCapture != null || grant != null || probing) stop() }
            "connected" -> { connected = true; connecting = false }
            "disconnected" -> { connected = false; connecting = false; stopCapture(); if (!stopping) { core?.close(); core = null; catalog.clear(); selectedDevice = null; dlnaId = null; dlnaIp = null } }
            "stopped" -> { stopping = false }
            "file.shared" -> {
                if (!filePending || stopping) return
                filePending = false
                media = body
                if (dlnaId != null) command("dlna.load", JSONObject().put("deviceId", dlnaId).put("url", body.getString("url")).put("title", "LanCast Video"))
                else pendingSession = core?.send("session.start", null, JSONObject().put("mode", "file").put("audioRequested", true))
            }
            "message" -> {
                val data = body.optJSONObject("body") ?: JSONObject()
                when (body.optString("type")) {
                    "session.accepted" -> {
                        if (pendingSession == null || body.optString("replyTo") != pendingSession) return
                        pendingSession = null
                        sessionId = body.getString("sessionId")
                        val permission = grant
                        if (permission != null) {
                            grant = null
                            val active = generation
                            peer = RtcPeer(context!!, null, { type, value -> android.os.Handler(android.os.Looper.getMainLooper()).post { if (generation == active) core?.send(type, sessionId, value) } }, { value ->
                                android.os.Handler(android.os.Looper.getMainLooper()).post {
                                    if (generation != active) return@post
                                    observer?.invoke(JSONObject().put("type", "media.status").put("body", JSONObject().put("status", value)))
                                    if (value == "CAPTURE_REVOKED" || value == "rtc_failed" || value == "AUDIO_NOT_CAPTURABLE" || value.endsWith("_FAILED")) stop()
                                }
                            }, recoveryEnabled = data.optString("rtcRecovery") == "replace-v1")
                            peer!!.startCapture(permission, withAudio, data.getJSONObject("selectedProfile"))
                        } else media?.let {
                            core?.send("file.load", sessionId, JSONObject(it.toString()).put("mediaId", java.util.UUID.randomUUID().toString()).put("durationMs", JSONObject.NULL))
                        }
                    }
                    "rtc.answer" -> peer?.receiveAnswer(data.getString("sdp"), data.getString("negotiationId"))
                    "rtc.ice" -> peer?.ice(data)
                    "rtc.restart" -> if (body.optString("sessionId") == sessionId) peer?.restart(data.getString("negotiationId"))
                    "session.stop" -> stopCapture()
                    "error" -> { stopCapture() }
                }
            }
        }
        observer?.invoke(event)
    }
    private fun stopCapture() { generation++; captureGate.cancel(); activeCapture = null; pendingSession = null; probing = false; liveCapture?.close(); liveCapture = null; liveUrl = null; peer?.close(); peer = null; grant = null; sessionId = null; media = null; val callback = stopService; stopService = null; callback?.invoke() }
    fun stop() {
        if (connecting) { close(); return }
        sessionId?.let { core?.send("session.stop", it, JSONObject().put("reason", "sender_stopped")) }
        core?.command("stop")
        stopping = core != null; filePending = false
        connected = false
        stopCapture()
    }
    fun close() {
        val pairing = connecting; connecting = false
        if (!pairing) stop() else stopCapture()
        connected = false; core?.close(); core = null; stopping = false; filePending = false
        scanGeneration++; scanning = false; releaseMulticast(); catalog.clear(); selectedDevice = null; dlnaId = null; dlnaIp = null; receiverAddress = ""
    }
}
