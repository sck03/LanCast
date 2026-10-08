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
import dev.lancast.receiver.contracts.Lease
import dev.lancast.receiver.contracts.Source
import dev.lancast.receiver.contracts.ReceiverOwnership

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
    private lateinit var networkLabel: TextView
    private lateinit var codeLabel: TextView
    private var automaticAddress = true
    private var fillingAddress = false
    private var connectionInfo = JSONObject()
    private var multicast: WifiManager.MulticastLock? = null
    private var approvalId: String? = null
    private var approvalDialog: AlertDialog? = null
    private var receivingLease: Lease? = null
    private lateinit var airplay: AirPlayFeature
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val root = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL; setPadding(24, 16, 24, 16) }
        root.addView(TextView(this).apply { text = "LanCast 接收端 · " + BuildConfig.FLAVOR; textSize = 24f })
        state = TextView(this).apply { text = "本机网络会自动选择，点击启动接收即可。返回键会停止接收。"; textSize = 16f }
        root.addView(state)
        networkLabel = TextView(this); root.addView(networkLabel)
        address = EditText(this).apply { setSingleLine(); hint = "特殊网络可手动填写本机 IPv4:8787" }
        val advanced = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL; visibility = View.GONE; addView(address) }
        advanced.addView(Button(this).apply { text = "重新自动选择网络"; setOnClickListener { if (core == null) { automaticAddress = true; refreshAddress() } } })
        root.addView(Button(this).apply { text = "高级网络设置"; setOnClickListener { advanced.visibility = if (advanced.visibility == View.VISIBLE) View.GONE else View.VISIBLE } })
        root.addView(advanced)
        address.addTextChangedListener(object : android.text.TextWatcher {
            override fun beforeTextChanged(s: CharSequence?, start: Int, count: Int, after: Int) {}
            override fun onTextChanged(s: CharSequence?, start: Int, before: Int, count: Int) { if (!fillingAddress) automaticAddress = false }
            override fun afterTextChanged(s: android.text.Editable?) {}
        })
        refreshAddress()
        val controls = LinearLayout(this)
        fun button(text: String, action: () -> Unit) { controls.addView(Button(this).apply { this.text = text; setOnClickListener { runCatching(action).onFailure { state.text = it.message } } }) }
        button("启动接收") { startReceiver() }
        button("刷新配对码") { core?.command("invite") }
        button("停止投屏") { if (!airplay.stopCurrent()) { core?.command("stop"); stopMedia() } }
        root.addView(controls)
        codeLabel = TextView(this).apply { textSize = 32f; setTextIsSelectable(true) }; root.addView(codeLabel)
        invitation = TextView(this).apply { textSize = 16f; setTextIsSelectable(true) }
        root.addView(invitation)
        display = FrameLayout(this)
        val airplayControls = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL }
        root.addView(airplayControls)
        airplay = AirPlayFeatureFactory.create(this, airplayControls, display)
        root.addView(display, LinearLayout.LayoutParams(-1, 0, 1f))
        setContentView(root)
    }
    private fun startReceiver() {
        if (core != null) return
        refreshAddress()
        check(dev.lancast.control.isLanIpv4(address.text.toString().substringBeforeLast(':'))) { "请先连接本地网络，或在高级设置选择有效地址" }
        connectionInfo = JSONObject(); codeLabel.text = ""; invitation.text = ""; address.isEnabled = false
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        val wifi = applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
        multicast = wifi.createMulticastLock("lancast-discovery").apply { setReferenceCounted(false); acquire() }
        core = ControlSession(::onEvent)
        core!!.command("listen", JSONObject().put("address", address.text.toString()).put("name", "LanCast TV").put("variant", if (BuildConfig.FLAVOR == "legacy") "legacy" else "standard"))
        state.text = "正在启动安全接收服务…"
    }
    private fun onEvent(event: JSONObject) {
        val body = event.optJSONObject("body") ?: JSONObject()
        when (event.getString("type")) {
            "receiver.ready" -> { connectionInfo = body; showInvitation(); state.text = "在发送端选择此设备并输入配对码，再在本屏允许连接" }
            "receiver.invite" -> { connectionInfo.put("invite", body.getString("invite")); showInvitation() }
            "pair.request" -> showApproval(body)
            "pair.closed" -> if (approvalId == body.optString("connectionId")) clearApproval()
            "session.started" -> {
                if (receivingLease == null) receivingLease = ReceiverOwnership.leases.acquire(Source.LANCAST, body.getString("sessionId"))
                if (receivingLease == null) { core?.command("stop"); state.text = "接收画面正被另一来源使用，请先停止当前投屏"; return }
                stopMedia(releaseLease = false)
                sessionId = body.getString("sessionId")
                val activeSession = sessionId
                if (body.getString("mode") == "mirror") {
                    val renderer = SurfaceViewRenderer(this)
                    display.addView(renderer, FrameLayout.LayoutParams(-1, -1))
                    peer = RtcPeer(this, renderer, { type, data -> runOnUiThread { if (sessionId == activeSession) core?.send(type, activeSession, data) } }, { value -> runOnUiThread { if (sessionId == activeSession) mediaStatus(value) } }, recoveryEnabled = body.optString("rtcRecovery") == "replace-v1")
                } else {
                    player = PlatformPlayer(this) { value -> if (sessionId == activeSession) mediaStatus(value) }
                    display.addView(player!!.view, FrameLayout.LayoutParams(-1, -1))
                }
                invitation.visibility = View.GONE
                codeLabel.visibility = View.GONE
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
            "error" -> {
                if (!connectionInfo.has("fingerprint")) {
                    core?.close(); core = null; address.isEnabled = true
                    multicast?.let { if (it.isHeld) it.release() }; multicast = null
                    window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
                }
                state.text = body.optString("code")
            }
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
    private fun showApproval(body: JSONObject) {
        val id = body.getString("connectionId")
        if (approvalId != null) {
            core?.command("approve", JSONObject().put("connectionId", id).put("accept", false))
            return
        }
        if (receivingLease == null) receivingLease = ReceiverOwnership.leases.acquire(Source.LANCAST, id)
        if (receivingLease == null) {
            core?.command("approve", JSONObject().put("connectionId", id).put("accept", false))
            state.text = "接收画面正被另一来源使用，请先停止当前投屏"
            return
        }
        approvalId = id
        fun answer(accept: Boolean) {
            if (approvalId != id) return
            core?.command("approve", JSONObject().put("connectionId", id).put("accept", accept))
            clearApproval(releaseLease = !accept)
            if (accept) {
                val reservation = receivingLease
                android.os.Handler(mainLooper).postDelayed({
                    if (sessionId == null && receivingLease == reservation) { ReceiverOwnership.leases.release(reservation); receivingLease = null; airplay.refreshDisplay() }
                }, 60_000)
            }
        }
        approvalDialog = AlertDialog.Builder(this).setTitle("允许此设备投屏？")
            .setMessage(body.optString("senderName") + "\n" + body.optString("address"))
            .setPositiveButton("允许") { _, _ -> answer(true) }
            .setNegativeButton("拒绝") { _, _ -> answer(false) }
            .setCancelable(false).show()
    }
    private fun clearApproval(releaseLease: Boolean = true) {
        approvalId = null
        approvalDialog?.dismiss()
        approvalDialog = null
        if (releaseLease && sessionId == null) { ReceiverOwnership.leases.release(receivingLease); receivingLease = null }
    }
    private fun showInvitation() {
        invitation.visibility = View.VISIBLE
        codeLabel.visibility = View.VISIBLE
        codeLabel.text = "配对码  " + connectionInfo.optString("invite").chunked(4).joinToString(" ")
        invitation.text = "配对码 2 分钟内单次有效。请核对发送端显示的完整指纹：\n" +
            connectionInfo.optString("fingerprint").chunked(8).chunked(4).joinToString("\n") { it.joinToString("  ") } + "\n本机地址：" + connectionInfo.optString("address")
    }
    private fun stopMedia(releaseLease: Boolean = true) {
        sessionId = null
        negotiationId = null; readySent = false
        peer?.close(); peer = null
        player?.close(); player = null
        display.removeAllViews()
        if (releaseLease) { ReceiverOwnership.leases.release(receivingLease); receivingLease = null; airplay.refreshDisplay() }
        invitation.visibility = View.VISIBLE
        codeLabel.visibility = View.VISIBLE
        state.text = "投屏已停止；新设备连接请刷新配对码"
    }
    private fun refreshAddress() {
        if (core != null) return
        if (automaticAddress) {
            fillingAddress = true
            address.setText(localAddresses(this).firstOrNull()?.let { "$it:8787" }.orEmpty())
            fillingAddress = false
        }
        networkLabel.text = if (address.text.isEmpty()) "未连接局域网，请连接 Wi-Fi 或网线" else "${if (automaticAddress) "自动选择网络" else "所选网络"} · ${address.text}"
    }
    override fun onStart() { super.onStart(); address.isEnabled = core == null; refreshAddress(); airplay.onStart() }
    override fun onStop() {
        airplay.onStop()
        clearApproval()
        stopMedia()
        core?.close(); core = null
        connectionInfo = JSONObject(); codeLabel.text = ""; invitation.text = ""
        multicast?.let { if (it.isHeld) it.release() }; multicast = null
        window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        super.onStop()
    }
    @Deprecated("Android back compatibility")
    override fun onBackPressed() { airplay.close(); super.onBackPressed() }
}
