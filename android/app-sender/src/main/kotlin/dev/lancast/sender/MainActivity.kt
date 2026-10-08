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
import dev.lancast.control.ReceiverHint
import org.json.JSONObject

class MainActivity : Activity() {
    private lateinit var state: TextView
    private lateinit var connection: ConnectionForm
    private val controls = mutableListOf<Pair<Button, () -> Boolean>>()
    private var externalRequest = false
    private var captureRequest: String? = null
    private lateinit var audio: CheckBox
    private var pendingAudio = false
    private var profileRequest: String? = null
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        captureRequest = savedInstanceState?.getString("captureRequest")
        externalRequest = savedInstanceState?.getBoolean("externalRequest", false) ?: false
        pendingAudio = savedInstanceState?.getBoolean("pendingAudio", false) ?: false
        val root = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL; setPadding(24, 16, 24, 16) }
        root.addView(TextView(this).apply { text = "LanCast · 局域网分享"; textSize = 26f })
        state = TextView(this).apply { text = "自有接收端支持屏幕与文件；DLNA 直播需先完成画面与声音测试。"; textSize = 16f }; root.addView(state)
        connection = ConnectionForm(this, ::scanDevices, ::selectDevice, { input ->
            SenderRuntime.localAddress = connection.localAddress(input.address.substringBeforeLast(':'))
            SenderRuntime.connect(input); state.text = "请在电视上允许连接"; updateControls()
        }, { state.text = it })
        root.addView(connection)
        root.addView(TextView(this).apply { text = "2  选择分享内容"; textSize = 20f })
        fun button(label: String, enabled: () -> Boolean = { true }, action: () -> Unit) {
            val button = Button(this).apply { text = label; setOnClickListener { runCatching(action).onFailure { state.text = it.message }; updateControls() } }
            controls.add(button to enabled); root.addView(button)
        }
        audio = CheckBox(this).apply { text = "分享允许采集的内部声音（不会启用麦克风）"; isChecked = true }; root.addView(audio)
        button("分享屏幕或应用", { SenderRuntime.connected && SenderRuntime.dlnaId == null && canStart() }) {
            check(SenderRuntime.connected && SenderRuntime.dlnaId == null) { "请先连接自有接收端" }
            pendingAudio = audio.isChecked
            externalRequest = true
            if (pendingAudio && checkSelfPermission(Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED)
                requestPermissions(arrayOf(Manifest.permission.RECORD_AUDIO), 30)
            else requestCapture()
        }
        button("测试电视兼容性", { SenderRuntime.dlnaId != null && canStart() }) {
            SenderRuntime.localAddress = connection.localAddress(checkNotNull(SenderRuntime.dlnaIp))
            SenderRuntime.beginProbe(this, audio.isChecked)
            state.text = "测试中：电视应显示红绿蓝交替画面，有声档应听到提示音。没有采集屏幕或内部声音。"
        }
        button("开始 DLNA 屏幕直播", { SenderRuntime.dlnaId != null && canStart() }) {
            check(SenderRuntime.dlnaId != null) { "先选择 DLNA 电视" }
            AlertDialog.Builder(this).setTitle("分享至 DLNA 电视")
                .setMessage("画面与允许采集的声音经局域网明文传输。当前配置需先通过合成测试，延迟取决于电视。")
                .setPositiveButton("继续并选择分享内容") { _, _ ->
                    runCatching {
                    SenderRuntime.localAddress = connection.localAddress(checkNotNull(SenderRuntime.dlnaIp))
                    pendingAudio = audio.isChecked
                    profileRequest = java.util.UUID.randomUUID().toString()
                    SenderRuntime.command("profile.check", JSONObject().put("deviceId", SenderRuntime.dlnaId).put("audio", pendingAudio).put("requestId", profileRequest))
                    }.onFailure { profileRequest = null; state.text = it.message }
                    updateControls()
                }.setNegativeButton("取消", null).show()
        }
        button("选择 MP4 视频播放", { (SenderRuntime.connected || SenderRuntime.dlnaId != null) && canStart() }) {
            check(SenderRuntime.connected || SenderRuntime.dlnaId != null) { "先选择自有接收端或 DLNA 电视" }
            externalRequest = true
            startActivityForResult(Intent(Intent.ACTION_OPEN_DOCUMENT).setType("video/mp4").addCategory(Intent.CATEGORY_OPENABLE), 20)
        }
        button("暂停", { SenderRuntime.filePlaying }) { SenderRuntime.playback("pause") }
        button("播放", { SenderRuntime.filePlaying }) { SenderRuntime.playback("play") }
        button("跳到 60 秒", { SenderRuntime.filePlaying }) { SenderRuntime.playback("seek", 60000) }
        button("停止") { profileRequest = null; externalRequest = false; SenderRuntime.stop() }
        button("断开连接") { profileRequest = null; SenderRuntime.close(); connection.clear(); state.text = "已断开，请刷新并重新选择电视" }
        button("电视不能安装 App？") { AlertDialog.Builder(this).setTitle("系统投屏与安装限制").setMessage(SystemGuide.TEXT).setPositiveButton("知道了", null).show() }
        setContentView(ScrollView(this).apply { addView(root) })
        SenderRuntime.ensureCore(this)
    }
    override fun onStart() {
        super.onStart(); SenderRuntime.observer = ::onEvent; updateControls()
        if (!SenderRuntime.selectionLocked && !externalRequest && SenderRuntime.catalog.devices.isEmpty()) runCatching(::scanDevices).onFailure { state.text = it.message }
    }
    override fun onStop() { profileRequest = null; if (SenderRuntime.probing) SenderRuntime.stop(); SenderRuntime.observer = null; super.onStop() }
    private fun canStart() = !SenderRuntime.busy && !SenderRuntime.connecting && !SenderRuntime.scanning && profileRequest == null && !externalRequest
    private fun scanDevices() {
        check(profileRequest == null && !externalRequest)
        val ip = connection.scanInterface()
        SenderRuntime.scan(this, ip); connection.clear(); state.text = "正在搜索同一网络的电视…"; updateControls()
    }
    private fun selectDevice(hint: ReceiverHint?) {
        if (SenderRuntime.selectionLocked || profileRequest != null || externalRequest) return
        SenderRuntime.selectedDevice = hint?.key
        SenderRuntime.dlnaId = hint?.takeIf { it.dlna }?.id; SenderRuntime.dlnaIp = hint?.takeIf { it.dlna }?.ip
        SenderRuntime.receiverAddress = hint?.takeUnless { it.dlna }?.address.orEmpty()
        updateControls()
    }
    private fun updateControls() {
        connection.render(SenderRuntime.catalog.devices, SenderRuntime.scanning, SenderRuntime.selectionLocked || profileRequest != null || externalRequest, SenderRuntime.selectedDevice)
        for ((button, enabled) in controls) button.isEnabled = enabled()
        audio.isEnabled = canStart()
    }
    private fun requestCapture() {
        runCatching {
            captureRequest = SenderRuntime.prepareCapture(); externalRequest = true; updateControls()
            startActivityForResult(getSystemService(MediaProjectionManager::class.java).createScreenCaptureIntent(), 10)
        }.onFailure { SenderRuntime.cancelCapture(captureRequest); captureRequest = null; externalRequest = false; state.text = it.message; updateControls() }
    }
    override fun onSaveInstanceState(state: Bundle) {
        state.putString("captureRequest", captureRequest); state.putBoolean("externalRequest", externalRequest)
        state.putBoolean("pendingAudio", pendingAudio)
        super.onSaveInstanceState(state)
    }
    override fun onRequestPermissionsResult(requestCode: Int, permissions: Array<out String>, results: IntArray) {
        super.onRequestPermissionsResult(requestCode, permissions, results)
        if (requestCode == 30) {
            if (results.firstOrNull() == PackageManager.PERMISSION_GRANTED) requestCapture()
            else AlertDialog.Builder(this).setMessage("内部声音权限未授予。是否明确选择静音分享？")
                .setPositiveButton("静音继续") { _, _ -> pendingAudio = false; requestCapture() }.setNegativeButton("取消") { _, _ -> externalRequest = false; updateControls() }.setOnCancelListener { externalRequest = false; updateControls() }.show()
        }
    }
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        super.onActivityResult(requestCode, resultCode, data)
        externalRequest = false
        if (resultCode != RESULT_OK || data == null) { if (requestCode == 10) SenderRuntime.cancelCapture(captureRequest); captureRequest = null; state.text = "操作已取消，没有启动采集"; updateControls(); return }
        if (requestCode == 10) {
            externalRequest = true
            runCatching { startForegroundService(Intent(this, CaptureService::class.java).putExtra("grant", data).putExtra("audio", pendingAudio).putExtra("requestId", checkNotNull(captureRequest) { "分享请求已过期，请重新授权" })) }
                .onFailure { SenderRuntime.cancelCapture(captureRequest); externalRequest = false; state.text = it.message }
            captureRequest = null
        }
        if (requestCode == 20) runCatching {
            check(!SenderRuntime.busy && (SenderRuntime.connected || SenderRuntime.dlnaId != null)) { "连接已失效，请重新选择电视" }
            val receiverIp = SenderRuntime.dlnaIp ?: SenderRuntime.receiverAddress.substringBeforeLast(':')
            val localIp = connection.localAddress(receiverIp)
            val uri = data.data ?: error("没有选择文件")
            val descriptor = contentResolver.openFileDescriptor(uri, "r") ?: error("无法打开文件")
            val fd = descriptor.detachFd(); descriptor.close()
            // Native side takes ownership of this detached read-only descriptor.
            SenderRuntime.command("file.share", JSONObject().put("fd", fd).put("allowedIp", SenderRuntime.dlnaIp ?: SenderRuntime.receiverAddress.substringBeforeLast(':'))
                .put("address", "$localIp:0").put("encrypted", SenderRuntime.dlnaId == null))
        }.onFailure { state.text = "文件无法读取：" + it.message }
        updateControls()
    }
    private fun onEvent(event: JSONObject) {
        val body = event.optJSONObject("body") ?: JSONObject()
        if (event.optString("type") in listOf("media.status", "error", "disconnected", "stopped")) externalRequest = false
        when (event.optString("type")) {
            "profile.checked" -> {
                if (profileRequest == null || body.optString("requestId") != profileRequest) return
                profileRequest = null
                if (body.optString("deviceId") != SenderRuntime.dlnaId) return
                if (!body.optBoolean("passed")) state.text = "请先完成当前电视、画面和声音配置的 DLNA 测试"
                else if (pendingAudio && checkSelfPermission(Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED) { externalRequest = true; requestPermissions(arrayOf(Manifest.permission.RECORD_AUDIO), 30) }
                else requestCapture()
            }
            "live.state" -> {
                state.text = when (body.optString("state")) { "pulling" -> "电视正在接收直播；实际画面和延迟请以电视为准"; "recovering" -> "电视拉流中断，正在进行一次恢复"; else -> "等待电视拉流或测试确认" }
                if (body.optString("state") == "awaiting_user_confirmation" && SenderRuntime.probing) {
                    AlertDialog.Builder(this).setTitle("电视是否正确播放？")
                        .setMessage("请在电视确认红绿蓝画面持续变化；有声档还需听到提示音。网络拉流成功本身不能证明画面和声音正常。")
                        .setPositiveButton("画面和所选声音正常") { _, _ -> SenderRuntime.command("probe.confirm", JSONObject().put("passed", true)) }
                        .setNegativeButton("无法正常播放") { _, _ -> SenderRuntime.command("probe.confirm", JSONObject().put("passed", false)) }
                        .setOnCancelListener { SenderRuntime.stop() }.show()
                }
            }
            "probe.saved" -> state.text = if (body.optBoolean("passed")) "测试档案已保存。现在可点击开始 DLNA 屏幕直播，再授权分享内容。" else "此配置未通过；仍可尝试 MP4 文件播放。"
            "connected" -> state.text = "安全连接已建立"
            "disconnected" -> state.text = "连接已断开，屏幕采集已停止；请重新配对"
            "error" -> state.text = body.optString("code")
            "media.status" -> if (!body.optString("status").startsWith("statistics:")) state.text = body.optString("status")
            "devices", "dlna.devices", "discovery.updated" -> if (!SenderRuntime.scanning) state.text = if (SenderRuntime.catalog.devices.isEmpty()) "未发现电视，请确认各端为当前版本并连接同一网络；网络设置可切换网卡" else "请选择电视，将自动获取地址和指纹"
            "dlna.state" -> state.text = "DLNA 指令已返回，实际画面请以电视为准"
            "message" -> if (body.optString("type") == "error") state.text = body.getJSONObject("body").optString("code")
        }
        updateControls()
    }
}
