package dev.lancast.sender

import android.Manifest
import android.app.Activity
import android.app.AlertDialog
import android.content.Intent
import android.content.pm.PackageManager
import android.media.projection.MediaProjectionManager
import android.os.Bundle
import android.widget.*
import dev.lancast.control.SystemGuide
import dev.lancast.control.localAddresses
import org.json.JSONObject

class MainActivity : Activity() {
    private lateinit var state: TextView
    private lateinit var address: EditText
    private lateinit var fingerprint: EditText
    private lateinit var invite: EditText
    private lateinit var local: EditText
    private lateinit var audio: CheckBox
    private var pendingAudio = false
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val root = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL; setPadding(24, 16, 24, 16) }
        root.addView(TextView(this).apply { text = "LanCast · 局域网分享"; textSize = 26f })
        state = TextView(this).apply { text = "自有接收端支持屏幕与文件；DLNA 设备只支持文件。"; textSize = 16f }; root.addView(state)
        fun field(hint: String, value: String = "") = EditText(this).apply { this.hint = hint; setText(value); setSingleLine(); root.addView(this) }
        local = field("本机 LAN IPv4", localAddresses().firstOrNull() ?: "")
        address = field("接收端 IPv4:8787")
        fingerprint = field("电视显示的完整 SHA-256 指纹")
        invite = field("电视显示的一次性邀请")
        fun button(label: String, action: () -> Unit) { root.addView(Button(this).apply { text = label; setOnClickListener { runCatching(action).onFailure { state.text = it.message } } }) }
        button("扫描自有接收端") { SenderRuntime.command("scan") }
        button("核对指纹并配对") {
            SenderRuntime.dlnaId = null; SenderRuntime.dlnaIp = null
            SenderRuntime.receiverAddress = address.text.toString().trim()
            SenderRuntime.command("connect", JSONObject().put("address", SenderRuntime.receiverAddress).put("fingerprint", fingerprint.text.toString().trim()).put("invite", invite.text.toString().trim()).put("name", android.os.Build.MODEL))
            state.text = "请在电视上确认本设备"
        }
        audio = CheckBox(this).apply { text = "分享允许采集的内部声音（不会启用麦克风）"; isChecked = true }; root.addView(audio)
        button("分享屏幕或应用") {
            check(SenderRuntime.connected && SenderRuntime.dlnaId == null) { "请先连接自有接收端" }
            pendingAudio = audio.isChecked
            if (pendingAudio && checkSelfPermission(Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED)
                requestPermissions(arrayOf(Manifest.permission.RECORD_AUDIO), 30)
            else requestCapture()
        }
        button("扫描 DLNA 电视（仅视频）") {
            SenderRuntime.command("dlna.scan", JSONObject().put("interface", local.text.toString().trim()))
            state.text = "扫描中；DLNA 文件通过局域网 HTTP 明文传输"
        }
        button("选择 MP4 视频播放") {
            check(SenderRuntime.connected || SenderRuntime.dlnaId != null) { "先选择自有接收端或 DLNA 电视" }
            startActivityForResult(Intent(Intent.ACTION_OPEN_DOCUMENT).setType("video/mp4").addCategory(Intent.CATEGORY_OPENABLE), 20)
        }
        button("暂停") { SenderRuntime.playback("pause") }
        button("播放") { SenderRuntime.playback("play") }
        button("跳到 60 秒") { SenderRuntime.playback("seek", 60000) }
        button("停止") { SenderRuntime.stop() }
        button("断开连接") { SenderRuntime.close(); state.text = "已断开，请重新配对" }
        button("电视不能安装 App？") { AlertDialog.Builder(this).setTitle("系统投屏与安装限制").setMessage(SystemGuide.TEXT).setPositiveButton("知道了", null).show() }
        setContentView(ScrollView(this).apply { addView(root) })
        SenderRuntime.ensureCore()
    }
    override fun onStart() { super.onStart(); SenderRuntime.observer = ::onEvent }
    override fun onStop() { SenderRuntime.observer = null; super.onStop() }
    private fun requestCapture() { startActivityForResult(getSystemService(MediaProjectionManager::class.java).createScreenCaptureIntent(), 10) }
    override fun onRequestPermissionsResult(requestCode: Int, permissions: Array<out String>, results: IntArray) {
        super.onRequestPermissionsResult(requestCode, permissions, results)
        if (requestCode == 30) {
            if (results.firstOrNull() == PackageManager.PERMISSION_GRANTED) requestCapture()
            else AlertDialog.Builder(this).setMessage("内部声音权限未授予。是否明确选择静音分享？")
                .setPositiveButton("静音继续") { _, _ -> pendingAudio = false; requestCapture() }.setNegativeButton("取消", null).show()
        }
    }
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        super.onActivityResult(requestCode, resultCode, data)
        if (resultCode != RESULT_OK || data == null) { state.text = "操作已取消，没有启动采集"; return }
        if (requestCode == 10) startForegroundService(Intent(this, CaptureService::class.java).putExtra("grant", data).putExtra("audio", pendingAudio))
        if (requestCode == 20) runCatching {
            val uri = data.data ?: error("没有选择文件")
            val descriptor = contentResolver.openFileDescriptor(uri, "r") ?: error("无法打开文件")
            val fd = descriptor.detachFd(); descriptor.close()
            // Native side takes ownership of this detached read-only descriptor.
            SenderRuntime.command("file.share", JSONObject().put("fd", fd).put("allowedIp", SenderRuntime.dlnaIp ?: SenderRuntime.receiverAddress.substringBeforeLast(':'))
                .put("address", local.text.toString().trim() + ":0").put("encrypted", SenderRuntime.dlnaId == null))
        }.onFailure { state.text = "文件无法读取：" + it.message }
    }
    private fun onEvent(event: JSONObject) {
        val body = event.optJSONObject("body") ?: JSONObject()
        when (event.optString("type")) {
            "connected" -> state.text = "安全连接已建立"
            "disconnected" -> state.text = "连接已断开，屏幕采集已停止；请重新配对"
            "error" -> state.text = body.optString("code")
            "media.status" -> if (!body.optString("status").startsWith("statistics:")) state.text = body.optString("status")
            "devices" -> {
                val devices = body.getJSONArray("devices")
                if (devices.length() == 0) state.text = "未发现设备，可手动输入地址"
                else AlertDialog.Builder(this).setTitle("选择接收端，随后核对指纹")
                    .setItems(Array(devices.length()) { devices.getJSONObject(it).getString("name") }) { _, n ->
                        val d = devices.getJSONObject(n); address.setText(d.getJSONArray("addresses").getString(0) + ":" + d.getInt("port"))
                    }.show()
            }
            "dlna.devices" -> {
                val devices = body.getJSONArray("devices")
                if (devices.length() == 0) state.text = "未发现有 AVTransport 服务的电视"
                else AlertDialog.Builder(this).setTitle("DLNA 视频设备")
                    .setItems(Array(devices.length()) { devices.getJSONObject(it).getString("name") }) { _, n ->
                        val d = devices.getJSONObject(n); SenderRuntime.dlnaId = d.getString("id"); SenderRuntime.dlnaIp = d.getString("ip")
                        state.text = "已选 DLNA：" + d.getString("name") + "（仅视频，HTTP 明文）"
                    }.show()
            }
            "dlna.state" -> state.text = "DLNA 指令已返回，实际画面请以电视为准"
            "message" -> if (body.optString("type") == "error") state.text = body.getJSONObject("body").optString("code")
        }
    }
}
