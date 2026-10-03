package dev.lancast.receiver

import android.app.Activity
import android.app.AlertDialog
import android.content.Context
import android.net.wifi.WifiManager
import android.os.Bundle
import android.view.View
import android.view.WindowManager
import android.widget.*
import dev.lancast.control.ControlSession
import dev.lancast.control.localAddresses
import dev.lancast.player.PlatformPlayer
import dev.lancast.media.RtcPeer
import org.json.JSONObject
import org.webrtc.SurfaceViewRenderer

class MainActivity : Activity() {
    private var core: ControlSession? = null
    private var peer: RtcPeer? = null
    private var player: PlatformPlayer? = null
    private var sessionId: String? = null
    private var negotiationId: String? = null
    private var readySent = false
    private lateinit var state: TextView
    private lateinit var invitation: TextView
    private lateinit var display: FrameLayout
    private lateinit var address: EditText
    private var connectionInfo = JSONObject()
    private var multicast: WifiManager.MulticastLock? = null
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val root = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL; setPadding(24, 16, 24, 16) }
        root.addView(TextView(this).apply { text = "LanCast 接收端 · " + BuildConfig.FLAVOR; textSize = 24f })
        state = TextView(this).apply { text = "选择本机局域网地址，启动接收。返回键会停止接收。"; textSize = 16f }
        root.addView(state)
        address = EditText(this).apply { setSingleLine(); setText((localAddresses().firstOrNull() ?: "") + ":8787"); hint = "本机 IPv4:8787" }
        root.addView(address)
        val controls = LinearLayout(this)
        fun button(text: String, action: () -> Unit) { controls.addView(Button(this).apply { this.text = text; setOnClickListener { runCatching(action).onFailure { state.text = it.message } } }) }
        button("启动接收") { startReceiver() }
        button("更新邀请") { core?.command("invite") }
        button("停止投屏") { core?.command("stop"); stopMedia() }
        root.addView(controls)
        invitation = TextView(this).apply { textSize = 13f; setTextIsSelectable(true) }
        root.addView(invitation)
        display = FrameLayout(this)
        root.addView(display, LinearLayout.LayoutParams(-1, 0, 1f))
        setContentView(root)
    }
    private fun startReceiver() {
        if (core != null) return
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        val wifi = applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
        multicast = wifi.createMulticastLock("lancast-discovery").apply { setReferenceCounted(false); acquire() }
        core = ControlSession(::onEvent)
        core!!.command("listen", JSONObject().put("address", address.text.toString()).put("name", "LanCast TV").put("variant", BuildConfig.FLAVOR))
        state.text = "正在启动安全接收服务…"
    }
    private fun onEvent(event: JSONObject) {
        val body = event.optJSONObject("body") ?: JSONObject()
        when (event.getString("type")) {
            "receiver.ready" -> { connectionInfo = body; showInvitation(); state.text = "等待发送端连接；新设备必须在本屏确认" }
            "receiver.invite" -> { connectionInfo.put("invite", body.getString("invite")); showInvitation() }
            "pair.request" -> AlertDialog.Builder(this).setTitle("允许此设备投屏？")
                .setMessage(body.optString("senderName") + "\n" + body.optString("address"))
                .setPositiveButton("允许") { _, _ -> core?.command("approve", JSONObject().put("connectionId", body.getString("connectionId")).put("accept", true)) }
                .setNegativeButton("拒绝") { _, _ -> core?.command("approve", JSONObject().put("connectionId", body.getString("connectionId")).put("accept", false)) }
                .setCancelable(false).show()
            "session.started" -> {
                stopMedia()
                sessionId = body.getString("sessionId")
                val activeSession = sessionId
                if (body.getString("mode") == "mirror") {
                    val renderer = SurfaceViewRenderer(this)
                    display.addView(renderer, FrameLayout.LayoutParams(-1, -1))
                    peer = RtcPeer(this, renderer, { type, data -> if (sessionId == activeSession) core?.send(type, activeSession, data) }, { value -> runOnUiThread { if (sessionId == activeSession) mediaStatus(value) } }, recoveryEnabled = body.optString("rtcRecovery") == "replace-v1")
                } else {
                    player = PlatformPlayer(this) { value -> if (sessionId == activeSession) mediaStatus(value) }
                    display.addView(player!!.view, FrameLayout.LayoutParams(-1, -1))
                }
                invitation.visibility = View.GONE
                state.text = "正在建立媒体连接…"
            }
            "message" -> if (body.optString("sessionId") == sessionId) when (body.getString("type")) {
                "rtc.offer" -> body.getJSONObject("body").let { negotiationId = it.getString("negotiationId"); readySent = false; peer?.receiveOffer(it.getString("sdp"), negotiationId!!) }
                "rtc.ice" -> peer?.ice(body.getJSONObject("body"))
                "file.load" -> body.getJSONObject("body").let { player?.load(it.getString("url"), it.getString("fingerprint")) }
                "playback.command" -> body.getJSONObject("body").let {
                    when (it.getString("action")) { "play" -> player?.play(); "pause" -> player?.pause(); "seek" -> player?.seek(it.getLong("positionMs")); "volume" -> player?.volume(it.getDouble("value").toFloat()) }
                }
                "session.stop" -> stopMedia()
            }
            "session.closed", "stopped" -> stopMedia()
            "error" -> state.text = body.optString("code")
        }
    }
    private fun mediaStatus(value: String) {
        if (sessionId == null) return
        when {
            value == "first_frame" || value == "ready" -> {
                state.text = "正在播放"
                if (!readySent) { readySent = true; core?.send("session.state", sessionId, JSONObject().put("state", "ready").put("negotiationId", negotiationId)) }
            }
            value.startsWith("statistics:") -> core?.send("statistics", sessionId, JSONObject(value.removePrefix("statistics:")))
            value == "ended" -> { core?.command("stop"); stopMedia() }
            value.endsWith("FAILED") || value == "MEDIA_UNSUPPORTED" || value == "MEDIA_SOURCE_FAILED" -> {
                core?.command("stop"); stopMedia(); state.text = value
            }
            else -> state.text = value
        }
    }
    private fun showInvitation() {
        invitation.visibility = View.VISIBLE
        invitation.text = "发送端核对完整 SHA-256 指纹后输入邀请（120 秒、单次有效）。\n地址：" + connectionInfo.optString("address") +
            "\n指纹：" + connectionInfo.optString("fingerprint") + "\n邀请：" + connectionInfo.optString("invite")
    }
    private fun stopMedia() {
        sessionId = null
        negotiationId = null; readySent = false
        peer?.close(); peer = null
        player?.close(); player = null
        display.removeAllViews()
        invitation.visibility = View.VISIBLE
        state.text = "投屏已停止；新设备连接请更新邀请"
    }
    override fun onStop() {
        stopMedia()
        core?.close(); core = null
        multicast?.let { if (it.isHeld) it.release() }; multicast = null
        window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        super.onStop()
    }
}
