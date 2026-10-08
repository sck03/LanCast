package dev.lancast.airplay

import android.Manifest
import android.app.Activity
import android.app.AlertDialog
import android.content.*
import android.content.pm.PackageManager
import android.os.Build
import android.os.IBinder
import android.view.SurfaceHolder
import android.view.SurfaceView
import android.view.View
import android.view.WindowManager
import android.widget.*
import dev.lancast.receiver.contracts.ReceiverOwnership
import dev.lancast.receiver.contracts.Source

/** UI binds only to local commands and immutable state; it never parses AirPlay or owns a decoder. */
class AirPlayPanel(private val activity: Activity, controls: LinearLayout, private val display: FrameLayout) {
    private var service: AirPlayService? = null
    private var bound = false
    private var visible = false
    private var rendering = false
    private var surface: SurfaceView? = null
    private var dialog: AlertDialog? = null
    private var pending = 0L
    private var videoWidth = 0
    private var videoHeight = 0
    private val toggle = Switch(activity).apply { text = "允许苹果系统屏幕镜像"; textSize = 18f; isChecked = false }
    private val name = EditText(activity).apply { setSingleLine(); setText("LanCast TV"); hint = "苹果设备看到的接收名称" }
    private val status = TextView(activity).apply { text = "按需开启，可在后台等待连接" }
    private val pin = TextView(activity).apply { textSize = 24f }
    private val observer: (AirPlayState) -> Unit = { show(it) }
    private val connection = object : ServiceConnection {
        override fun onServiceConnected(component: ComponentName, binder: IBinder) {
            service = (binder as AirPlayService.LocalBinder).service
            service?.observe(observer)
            refreshDisplay()
        }
        override fun onServiceDisconnected(component: ComponentName) {
            service = null; rendering = true; toggle.isChecked = false; rendering = false
            status.text = "接收服务已停止，请手动重新开启"
            dismiss()
        }
    }
    init {
        display.addOnLayoutChangeListener { _, _, _, _, _, _, _, _, _ -> fitSurface() }
        controls.addView(toggle); controls.addView(name); controls.addView(status); controls.addView(pin)
        toggle.setOnCheckedChangeListener { _, enabled ->
            if (!rendering) {
                if (enabled) {
                    if (Build.VERSION.SDK_INT >= 33 && activity.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) activity.requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), 7301)
                    runCatching { AirPlayService.enable(activity, name.text.toString()) }.onFailure { status.text = it.message; rendering = true; toggle.isChecked = false; rendering = false }
                } else service?.disable()
            }
        }
    }
    fun onStart() {
        visible = true
        if (!bound) bound = activity.bindService(Intent(activity, AirPlayService::class.java), connection, Context.BIND_AUTO_CREATE)
        refreshDisplay()
    }
    fun onStop() {
        visible = false; service?.attachDisplay(null); dismiss()
        service?.unobserve(observer)
        surface?.let { display.removeView(it) }; surface = null
        if (bound) { activity.unbindService(connection); bound = false }
        service = null
        activity.window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
    }
    fun close() { service?.disable(); onStop() }
    fun stopCurrent(): Boolean {
        if (ReceiverOwnership.leases.snapshot()?.source != Source.AIRPLAY) return false
        service?.stopCurrent(); return true
    }
    fun refreshDisplay() {
        if (!visible || ReceiverOwnership.leases.snapshot()?.source == Source.LANCAST) return
        if (surface?.parent == display) return
        surface = SurfaceView(activity).also { view ->
            view.holder.addCallback(object : SurfaceHolder.Callback {
                override fun surfaceCreated(holder: SurfaceHolder) { if (visible) service?.attachDisplay(holder.surface) }
                override fun surfaceChanged(holder: SurfaceHolder, format: Int, width: Int, height: Int) { if (visible) service?.attachDisplay(holder.surface) }
                override fun surfaceDestroyed(holder: SurfaceHolder) { service?.attachDisplay(null) }
            })
            display.addView(view, FrameLayout.LayoutParams(-1, -1))
        }
        fitSurface()
    }
    private fun fitSurface() {
        val view = surface ?: return
        if (view.parent != display || videoWidth <= 0 || videoHeight <= 0 || display.width <= 0 || display.height <= 0) return
        val scale = minOf(display.width.toDouble() / videoWidth, display.height.toDouble() / videoHeight)
        val width = (videoWidth * scale).toInt().coerceAtLeast(1)
        val height = (videoHeight * scale).toInt().coerceAtLeast(1)
        if (view.layoutParams.width != width || view.layoutParams.height != height) view.layoutParams = FrameLayout.LayoutParams(width, height, android.view.Gravity.CENTER)
    }
    private fun show(state: AirPlayState) {
        rendering = true; toggle.isChecked = state.enabled; rendering = false
        name.isEnabled = !state.enabled
        if (state.enabled && name.text.toString() != state.name) name.setText(state.name)
        status.text = state.message
        videoWidth = state.videoWidth; videoHeight = state.videoHeight
        pin.text = if (state.pin.isEmpty()) "" else "苹果配对码  ${state.pin.take(3)}-${state.pin.substring(3, 5)}-${state.pin.takeLast(3)}"
        if (state.playing) activity.window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        else if (ReceiverOwnership.leases.snapshot()?.source != Source.LANCAST) activity.window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        refreshDisplay()
        fitSurface()
        val request = state.request
        if (request == null) { dismiss(); return }
        if (!visible || pending == request.session) return
        dismiss(); pending = request.session
        dialog = AlertDialog.Builder(activity).setTitle("允许苹果设备投屏？")
            .setMessage("${request.name}\n${request.peer}\n允许后会显示该设备的屏幕并播放声音。")
            .setPositiveButton("允许") { _, _ -> service?.approve(request.session, true); dismiss() }
            .setNegativeButton("拒绝") { _, _ -> service?.approve(request.session, false); dismiss() }
            .setCancelable(false).show()
    }
    private fun dismiss() { dialog?.dismiss(); dialog = null; pending = 0 }
}
