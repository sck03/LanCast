package dev.lancast.sender

/** All access is serialized by DlnaCapture's codec worker. */
class TsMux(width: Int, height: Int, audio: Boolean, sink: Sink) : AutoCloseable {
    fun interface Sink { fun onTs(bytes: ByteArray): Int }
    companion object { init { System.loadLibrary("lancast_media") } }
    private external fun create(width: Int, height: Int, audio: Boolean, sink: Sink): Long
    private external fun write(handle: Long, audio: Boolean, bytes: ByteArray, ptsUs: Long): Int
    private external fun destroy(handle: Long)
    private var handle = create(width, height, audio, sink).also { check(it != 0L) { "TS_INIT_FAILED" } }
    fun video(bytes: ByteArray, ptsUs: Long) { check(write(handle, false, bytes, ptsUs) == 0) { "TS_VIDEO_FAILED" } }
    fun audio(bytes: ByteArray, ptsUs: Long) { check(write(handle, true, bytes, ptsUs) == 0) { "TS_AUDIO_FAILED" } }
    override fun close() { if (handle != 0L) { destroy(handle); handle = 0 } }
}
