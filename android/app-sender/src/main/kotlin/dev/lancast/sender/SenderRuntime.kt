package dev.lancast.sender

import android.content.Context
import android.content.Intent
import dev.lancast.control.ControlSession
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
    fun ensureCore(context: Context? = null) {
        if (context != null) profilesPath = java.io.File(context.filesDir, "receiver-profiles.json").absolutePath
        if (core == null) { core = ControlSession(::onEvent); profilesPath?.let { core!!.command("profiles.open", JSONObject().put("path", it)) } }
    }
    fun command(op: String, body: JSONObject = JSONObject()) { ensureCore(); core!!.command(op, body) }
    fun beginCapture(context: Context, permission: Intent, audio: Boolean, stopService: () -> Unit) {
        check(connected || dlnaId != null)
        check(peer == null && liveCapture == null && grant == null) { "先停止当前分享" }
        generation++
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
        check(liveCapture == null || action == "stop") { "直播只支持停止，不能暂停或跳转" }
        if (dlnaId != null) command("dlna.command", JSONObject().put("deviceId", dlnaId).put("action", action).put("positionMs", positionMs))
        else core?.send("playback.command", sessionId, JSONObject().put("action", action).put("positionMs", positionMs))
    }
    private fun onEvent(event: JSONObject) {
        val body = event.optJSONObject("body") ?: JSONObject()
        if (event.optString("type").startsWith("live.") && body.has("generation") && body.optLong("generation", -1) != generation) return
        when (event.optString("type")) {
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
            "error" -> if (peer != null || liveCapture != null || grant != null || probing) stop()
            "connected" -> connected = true
            "disconnected" -> { connected = false; stopCapture(); core?.close(); core = null }
            "file.shared" -> {
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
    private fun stopCapture() { generation++; pendingSession = null; probing = false; liveCapture?.close(); liveCapture = null; liveUrl = null; peer?.close(); peer = null; grant = null; sessionId = null; media = null; val callback = stopService; stopService = null; callback?.invoke() }
    fun stop() {
        sessionId?.let { core?.send("session.stop", it, JSONObject().put("reason", "sender_stopped")) }
        if (dlnaId != null) playback("stop")
        core?.command("stop")
        connected = false
        stopCapture()
    }
    fun close() { stop(); connected = false; core?.close(); core = null }
}
