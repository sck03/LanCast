package dev.lancast.player

import android.content.Context
import android.media.AudioAttributes
import android.media.MediaPlayer
import android.view.SurfaceHolder
import android.view.SurfaceView
import android.view.View
import android.widget.Button
import android.widget.LinearLayout
import dev.lancast.control.ControlSession
import org.json.JSONObject

/** API21 system player. HTTPS pinning and Range proxying live in the Rust adapter. */
class PlatformPlayer(context: Context, private val event: (String) -> Unit) : FilePlayer {
    private val surface = SurfaceView(context)
    private val root = LinearLayout(context).apply {
        orientation = LinearLayout.VERTICAL
        addView(surface, LinearLayout.LayoutParams(-1, 0, 1f))
        addView(LinearLayout(context).apply {
            fun button(label: String, action: () -> Unit) {
                addView(Button(context).apply { text = label; setOnClickListener { action() } })
            }
            button("播放") { play() }; button("暂停") { pause() }; button("停止") { stop(); event("ended") }
        })
    }
    override val view: View get() = root
    private var media: MediaPlayer? = null
    private var bridge: ControlSession? = null
    private var localUrl: String? = null
    private var hasSurface = false
    private var ready = false
    private var generation = 0L
    init {
        surface.holder.addCallback(object : SurfaceHolder.Callback {
            override fun surfaceCreated(holder: SurfaceHolder) { hasSurface = true; prepare() }
            override fun surfaceChanged(holder: SurfaceHolder, format: Int, width: Int, height: Int) {}
            override fun surfaceDestroyed(holder: SurfaceHolder) { hasSurface = false; close() }
        })
    }
    override fun load(url: String, fingerprint: String) {
        close()
        val current = generation
        bridge = ControlSession { message ->
            if (current == generation) when (message.optString("type")) {
                "bridge.ready" -> { localUrl = message.getJSONObject("body").getString("url"); prepare() }
                "error" -> { close(); event("MEDIA_SOURCE_FAILED") }
            }
        }.also { it.command("bridge.create", JSONObject().put("url", url).put("fingerprint", fingerprint)) }
    }
    private fun prepare() {
        val url = localUrl ?: return
        if (!hasSurface || media != null) return
        val current = generation
        runCatching {
            media = MediaPlayer().also { player ->
                player.setAudioAttributes(AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_MEDIA).setContentType(AudioAttributes.CONTENT_TYPE_MOVIE).build())
                player.setDisplay(surface.holder)
                player.setOnPreparedListener { if (current == generation) { ready = true; it.start(); event("ready") } }
                player.setOnCompletionListener { if (current == generation) event("ended") }
                player.setOnErrorListener { _, _, _ -> if (current == generation) { close(); event("MEDIA_UNSUPPORTED") }; true }
                player.setDataSource(url); player.prepareAsync()
            }
        }.onFailure { close(); event("MEDIA_UNSUPPORTED") }
    }
    override fun play() { if (ready) media?.start() }
    override fun pause() { if (ready) media?.pause() }
    @Suppress("DEPRECATION")
    override fun seek(positionMs: Long) { require(positionMs in 0..Int.MAX_VALUE.toLong()); if (ready) media?.seekTo(positionMs.toInt()) }
    override fun volume(value: Float) { require(value in 0f..1f); media?.setVolume(value, value) }
    override fun stop() = close()
    override fun close() {
        generation++; ready = false; localUrl = null
        media?.release(); media = null
        bridge?.close(); bridge = null
    }
}
