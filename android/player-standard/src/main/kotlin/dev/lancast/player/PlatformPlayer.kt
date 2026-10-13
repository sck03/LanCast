package dev.lancast.player

import android.content.Context
import android.view.View
import androidx.media3.common.AudioAttributes
import androidx.media3.common.C
import androidx.media3.common.MediaItem
import androidx.media3.common.Player
import androidx.media3.common.PlaybackException
import androidx.media3.datasource.okhttp.OkHttpDataSource
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.exoplayer.source.DefaultMediaSourceFactory
import androidx.media3.ui.PlayerView
import dev.lancast.control.pinnedHttp

/** Shared implementation, compiled separately against each flavor's Media3 version. */
class PlatformPlayer(private val context: Context, private val event: (String) -> Unit) : FilePlayer {
    private val surface = PlayerView(context).apply { useController = true }
    override val view: View get() = surface
    private var player: ExoPlayer? = null
    override fun load(url: String, fingerprint: String) {
        require(url.startsWith("https://"))
        close()
        val factory = DefaultMediaSourceFactory(OkHttpDataSource.Factory(pinnedHttp(fingerprint)))
        player = ExoPlayer.Builder(context).setMediaSourceFactory(factory).build().also {
            it.setAudioAttributes(AudioAttributes.Builder().setUsage(C.USAGE_MEDIA).setContentType(C.AUDIO_CONTENT_TYPE_MOVIE).build(), true)
            surface.player = it
            it.addListener(object : Player.Listener {
                override fun onPlaybackStateChanged(state: Int) {
                    if (state == Player.STATE_READY) event("ready")
                    if (state == Player.STATE_ENDED) event("ended")
                }
                override fun onPlayerError(error: PlaybackException) { event("MEDIA_UNSUPPORTED") }
            })
            it.setMediaItem(MediaItem.fromUri(url)); it.prepare(); it.playWhenReady = true
        }
    }
    override fun play() { player?.play() }
    override fun pause() { player?.pause() }
    override fun seek(positionMs: Long) { require(positionMs >= 0); player?.seekTo(positionMs) }
    override fun volume(value: Float) { require(value in 0f..1f); player?.volume = value }
    override fun stop() { close() }
    override fun close() { surface.player = null; player?.release(); player = null }
}
