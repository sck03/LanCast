package dev.lancast.player

import android.view.View
import java.io.Closeable

interface FilePlayer : Closeable {
    val view: View
    fun load(url: String, fingerprint: String)
    fun play()
    fun pause()
    fun seek(positionMs: Long)
    fun volume(value: Float)
    fun stop()
}
