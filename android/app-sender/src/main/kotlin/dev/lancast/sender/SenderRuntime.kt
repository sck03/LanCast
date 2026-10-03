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
    private var grant: Intent? = null
    private var context: Context? = null
    private var withAudio = false
    private var media: JSONObject? = null
    var receiverAddress = ""
    var connected = false
        private set
    var dlnaId: String? = null
    var dlnaIp: String? = null
    private var stopService: (() -> Unit)? = null
    fun ensureCore() { if (core == null) core = ControlSession(::onEvent) }
    fun command(op: String, body: JSONObject = JSONObject()) { ensureCore(); core!!.command(op, body) }
    fun beginCapture(context: Context, permission: Intent, audio: Boolean, stopService: () -> Unit) {
        check(connected || dlnaId != null)
        check(peer == null && liveCapture == null && grant == null) { "先停止当前分享" }
        generation++
        this.context = context.applicationContext; grant = permission; withAudio = audio; this.stopService = stopService
        if (dlnaId != null) command("live.create", JSONObject().put("address", "$localAddress:0").put("allowedIp", dlnaIp))
        else core!!.send("session.start", null, JSONObject().put("mode", "mirror").put("audioRequested", audio))
    }
    fun playback(action: String, positionMs: Long = 0) {
        check(liveCapture == null || action == "stop") { "直播只支持停止，不能暂停或跳转" }
        if (dlnaId != null) command("dlna.command", JSONObject().put("deviceId", dlnaId).put("action", action).put("positionMs", positionMs))
        else core?.send("playback.command", sessionId, JSONObject().put("action", action).put("positionMs", positionMs))
    }
    private fun onEvent(event: JSONObject) {
        val body = event.optJSONObject("body") ?: JSONObject()
        when (event.optString("type")) {
            "live.created" -> {
                val permission = grant ?: return
                grant = null; liveUrl = body.getString("url")
                val active = generation
                liveCapture = DlnaCapture(context!!, { bytes -> core?.writeTs(bytes) ?: -1 }) { value ->
                    android.os.Handler(android.os.Looper.getMainLooper()).post {
                        if (generation == active) {
                            if (value == "dlna_capture_started") command("dlna.load", JSONObject().put("deviceId", dlnaId).put("url", liveUrl).put("title", "LanCast Live").put("live", true))
                            else if (value.endsWith("FAILED") || value.contains("REQUIRED") || value == "CAPTURE_REVOKED") stop()
                            observer?.invoke(JSONObject().put("type", "media.status").put("body", JSONObject().put("status", value)))
                        }
                    }
                }.also { it.start(permission, withAudio) }
            }
            "error" -> if (liveCapture != null || grant != null) stop()
            "connected" -> connected = true
            "disconnected" -> { connected = false; stopCapture(); core?.close(); core = null }
            "file.shared" -> {
                media = body
                if (dlnaId != null) command("dlna.load", JSONObject().put("deviceId", dlnaId).put("url", body.getString("url")).put("title", "LanCast Video"))
                else core?.send("session.start", null, JSONObject().put("mode", "file").put("audioRequested", true))
            }
            "message" -> {
                val data = body.optJSONObject("body") ?: JSONObject()
                when (body.optString("type")) {
                    "session.accepted" -> {
                        sessionId = body.getString("sessionId")
                        val permission = grant
                        if (permission != null) {
                            grant = null
                            peer = RtcPeer(context!!, null, { type, value -> core?.send(type, sessionId, value) }, { value ->
                                android.os.Handler(android.os.Looper.getMainLooper()).post {
                                    observer?.invoke(JSONObject().put("type", "media.status").put("body", JSONObject().put("status", value)))
                                    if (value == "CAPTURE_REVOKED" || value == "rtc_failed" || value == "AUDIO_NOT_CAPTURABLE" || value.endsWith("_FAILED")) stop()
                                }
                            })
                            peer!!.startCapture(permission, withAudio, data.getJSONObject("selectedProfile"))
                        } else media?.let {
                            core?.send("file.load", sessionId, JSONObject(it.toString()).put("mediaId", java.util.UUID.randomUUID().toString()).put("durationMs", JSONObject.NULL))
                        }
                    }
                    "rtc.answer" -> peer?.receiveAnswer(data.getString("sdp"), data.getString("negotiationId"))
                    "rtc.ice" -> peer?.ice(data)
                    "session.stop" -> stopCapture()
                    "error" -> { stopCapture() }
                }
            }
        }
        observer?.invoke(event)
    }
    private fun stopCapture() { generation++; liveCapture?.close(); liveCapture = null; liveUrl = null; peer?.close(); peer = null; grant = null; sessionId = null; media = null; val callback = stopService; stopService = null; callback?.invoke() }
    fun stop() {
        sessionId?.let { core?.send("session.stop", it, JSONObject().put("reason", "sender_stopped")) }
        if (dlnaId != null) playback("stop")
        core?.command("stop")
        connected = false
        stopCapture()
    }
    fun close() { stop(); connected = false; core?.close(); core = null }
}
