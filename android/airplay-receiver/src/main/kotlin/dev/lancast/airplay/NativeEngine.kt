package dev.lancast.airplay

/** Narrow implementation boundary; no Activity, decoder or protocol objects cross JNI. */
internal class NativeEngine(private val listener: Listener) : AutoCloseable {
    interface Listener {
        fun event(session: Long, type: Int, text: String)
        fun videoConfig(session: Long, width: Int, height: Int, sps: ByteArray, pps: ByteArray): Boolean
        fun video(session: Long, pts: Long, key: Boolean, data: ByteArray): Boolean
        fun audioConfig(session: Long, codec: Int, rate: Int, channels: Int, spf: Int): Boolean
        fun audio(session: Long, pts: Long, data: ByteArray): Boolean
    }
    @Volatile private var handle = 0L
    fun start(address: String, name: String, seed: ByteArray, pin: String) {
        check(handle == 0L)
        handle = startNative(address, name, seed, pin)
        check(handle != 0L) { "AirPlay 引擎无法启动" }
    }
    fun approve(session: Long, accept: Boolean) { val h = handle; if (h != 0L) approveNative(h, session, accept) }
    fun stopSession(session: Long) { val h = handle; if (h != 0L) stopSessionNative(h, session) }
    override fun close() { val h = handle; handle = 0; if (h != 0L) stopNative(h) }
    @Suppress("unused") fun onEvent(session: Long, type: Int, text: String) = listener.event(session, type, text)
    @Suppress("unused") fun onVideoConfig(session: Long, width: Int, height: Int, sps: ByteArray, pps: ByteArray) = listener.videoConfig(session, width, height, sps, pps)
    @Suppress("unused") fun onVideo(session: Long, pts: Long, key: Boolean, data: ByteArray) = listener.video(session, pts, key, data)
    @Suppress("unused") fun onAudioConfig(session: Long, codec: Int, rate: Int, channels: Int, spf: Int) = listener.audioConfig(session, codec, rate, channels, spf)
    @Suppress("unused") fun onAudio(session: Long, pts: Long, data: ByteArray) = listener.audio(session, pts, data)
    private external fun startNative(address: String, name: String, seed: ByteArray, pin: String): Long
    private external fun approveNative(handle: Long, session: Long, accept: Boolean)
    private external fun stopSessionNative(handle: Long, session: Long)
    private external fun stopNative(handle: Long)
    companion object { init { System.loadLibrary("lancast_airplay") } }
}
