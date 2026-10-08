package dev.lancast.sender

import android.app.Activity
import android.app.AlertDialog
import android.view.View
import android.widget.*
import dev.lancast.control.*

/** Native form only; the coordinator owns discovery, pairing and capture. */
class ConnectionForm(private val activity: Activity, private val scan: () -> Unit,
                     private val selected: (ReceiverHint?) -> Unit,
                     private val connect: (PairingInput) -> Unit,
                     private val report: (String) -> Unit) : LinearLayout(activity) {
    val invite = EditText(activity)
    private var current: ReceiverHint? = null
    private val networkText = TextView(activity)
    private val devices = Spinner(activity)
    private val networks = Spinner(activity)
    private val advanced = LinearLayout(activity)
    private val scanButton = Button(activity)
    private val connectButton = Button(activity)
    private var hints = emptyList<ReceiverHint>()
    private var addresses = emptyList<String>()
    private var key: String? = null
    private var updating = false
    private var locked = false
    private var wasScanning = false
    private var manualIp: String? = null
    init {
        orientation = VERTICAL
        addView(TextView(activity).apply { text = "1  选择电视"; textSize = 20f })
        addView(networkText); addView(devices)
        scanButton.text = "刷新电视"; scanButton.setOnClickListener { guarded(scan) }; addView(scanButton)
        invite.hint = "电视上的 8 位配对码"; invite.setSingleLine(); invite.textSize = 22f
        invite.inputType = android.text.InputType.TYPE_CLASS_NUMBER
        invite.filters = arrayOf(android.text.InputFilter.LengthFilter(16))
        invite.importantForAutofill = View.IMPORTANT_FOR_AUTOFILL_NO; addView(invite)
        connectButton.text = "连接电视"; connectButton.setOnClickListener { guarded(::confirm) }; addView(connectButton)
        addView(Button(activity).apply { text = "网络设置"; setOnClickListener { advanced.visibility = if (advanced.visibility == View.VISIBLE) View.GONE else View.VISIBLE } })
        advanced.orientation = VERTICAL; advanced.visibility = View.GONE
        advanced.addView(TextView(activity).apply { text = "本机网络（默认自动选择）" }); advanced.addView(networks)
        addView(advanced)
        addView(TextView(activity).apply { text = "地址和指纹会自动填入；连接时请核对电视身份。普通 DLNA 电视不需要配对码。"; textSize = 14f })
        devices.onItemSelectedListener = object : AdapterView.OnItemSelectedListener {
            override fun onNothingSelected(parent: AdapterView<*>?) {}
            override fun onItemSelected(parent: AdapterView<*>?, view: View?, position: Int, id: Long) {
                if (updating || locked || position != devices.selectedItemPosition) return
                if (position == 0) { if (current != null) { current = null; key = null; invite.setText(""); selected(null) }; return }
                hints.getOrNull(position - 1)?.let { if (it.key != key) applySelection(it) }
            }
        }
        networks.onItemSelectedListener = object : AdapterView.OnItemSelectedListener {
            override fun onNothingSelected(parent: AdapterView<*>?) {}
            override fun onItemSelected(parent: AdapterView<*>?, view: View?, position: Int, id: Long) {
                if (updating || locked) return
                val next = if (position == 0) null else addresses.getOrNull(position - 1)
                if (next == manualIp) return
                manualIp = next
                showNetwork(); guarded(scan)
            }
        }
        refreshNetworks()
    }
    private fun guarded(action: () -> Unit) { runCatching(action).onFailure { report(it.message ?: "操作失败，请重试") } }
    fun scanInterface(): String { refreshNetworks(); return manualIp ?: addresses.firstOrNull().orEmpty() }
    fun localAddress(receiverIp: String): String = localAddressFor(activity, receiverIp, manualIp).also {
        networkText.text = "${if (manualIp == null) "自动选择网络" else "所选网络"} · $it"
    }
    private fun refreshNetworks() {
        addresses = localAddresses(activity)
        manualIp = manualIp?.takeIf { it in addresses }
        updating = true
        networks.adapter = ArrayAdapter(activity, android.R.layout.simple_spinner_dropdown_item, listOf("自动选择") + addresses)
        networks.setSelection(manualIp?.let { addresses.indexOf(it) + 1 } ?: 0)
        updating = false; showNetwork()
    }
    private fun showNetwork() { networkText.text = (manualIp ?: addresses.firstOrNull())?.let { "本机网络 · $it" } ?: "未连接局域网，请先连接与电视相同的 Wi-Fi" }
    fun render(found: List<ReceiverHint>, scanning: Boolean, locked: Boolean, selection: String?) {
        val changed = hints != found
        val completed = wasScanning && !scanning
        wasScanning = scanning
        this.locked = locked || scanning; hints = found
        if (changed || completed || key != selection || devices.adapter == null) {
            updating = true
            if (key != selection) {
                val hint = hints.firstOrNull { it.key == selection }
                key = selection; current = hint; invite.setText("")
            }
            devices.adapter = ArrayAdapter(activity, android.R.layout.simple_spinner_dropdown_item, listOf(if (scanning) "正在搜索电视…" else "请选择电视") + hints.map { it.label })
            devices.setSelection(hints.indexOfFirst { it.key == selection }.let { if (it < 0) 0 else it + 1 })
            updating = false
        }
        for (input in listOf<View>(devices, networks, scanButton)) input.isEnabled = !this.locked
        val dlna = hints.firstOrNull { it.key == selection }?.dlna == true
        invite.isEnabled = !this.locked && !dlna; connectButton.isEnabled = !this.locked && !dlna && current != null
        scanButton.text = if (scanning) "搜索中…" else "刷新电视"
        if ((changed || completed) && !this.locked && selection == null && hints.size == 1) applySelection(hints[0])
    }
    private fun applySelection(hint: ReceiverHint) {
        updating = true; key = hint.key; current = hint; invite.setText("")
        devices.setSelection(hints.indexOf(hint) + 1); updating = false
        selected(hint)
        report(if (hint.dlna) "已选择普通电视，可播放 MP4；屏幕直播前请测试兼容性。" else "已获取电视地址和指纹，请输入电视上的配对码。")
    }
    fun clear() {
        updating = true; key = null; current = null; invite.setText(""); devices.setSelection(0); updating = false
    }
    private fun confirm() {
        check(!locked) { "请先停止当前分享或等待搜索结束" }
        val target = checkNotNull(current?.takeUnless { it.dlna }) { "请搜索并选择当前版本的接收端" }
        val endpoint = target.address
        val pin = checkNotNull(normalizedFingerprint(target.fingerprint)) { "接收端身份信息无效，请重新搜索" }
        val code = invite.text.toString().filterNot { it.isWhitespace() }
        check(code.length == 8 && code.all { it in '0'..'9' }) { "请输入电视上的 8 位数字配对码" }
        val input = PairingInput(endpoint, pin, code)
        AlertDialog.Builder(activity).setTitle("核对电视身份")
            .setMessage("请确认以下完整指纹与电视一致：\n\n${pin.chunked(8).chunked(4).joinToString("\n") { it.joinToString("  ") }}\n\n$endpoint")
            .setNegativeButton("取消", null).setPositiveButton("一致，连接") { _, _ -> guarded { connect(input); invite.setText("") } }.show()
    }
}
