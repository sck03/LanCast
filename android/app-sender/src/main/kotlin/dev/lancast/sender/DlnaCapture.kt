package dev.lancast.sender

import android.app.Activity
import android.content.Context
import android.content.Intent
import android.hardware.display.DisplayManager
import android.hardware.display.VirtualDisplay
import android.media.*
import android.media.projection.MediaProjection
import android.media.projection.MediaProjectionManager
import android.os.Handler
import android.os.HandlerThread
import android.view.Surface
import java.util.concurrent.atomic.AtomicBoolean

/** One MediaProjection, one video Surface and one optional playback-only AudioRecord. */
class DlnaCapture(private val context: Context, private val sink: TsMux.Sink, private val status: (String) -> Unit) : AutoCloseable {
    private val closed = AtomicBoolean(false)
    private val thread = HandlerThread("lancast-dlna-codecs").apply { start() }
    private val worker = Handler(thread.looper)
    private var projection: MediaProjection? = null
    private var display: VirtualDisplay? = null
    private var surface: Surface? = null
    private var video: MediaCodec? = null
    private var audio: MediaCodec? = null
    @Volatile private var record: AudioRecord? = null
    private var audioThread: Thread? = null
    private var mux: TsMux? = null
    private var probeFrames: ProbeFrames? = null
    private var config = ByteArray(0)
    private var baseUs = 0L
    private var width = 1280
    private var height = 720
    private fun fail(reason: String) { if (!closed.get()) { status(reason); close() } }
    fun start(permission: Intent, internalAudio: Boolean) {
        startInternal(permission, internalAudio)
    }
    fun startProbe(audio: Boolean) { startInternal(null, audio) }
    private fun startInternal(permission: Intent?, internalAudio: Boolean) {
        worker.post {
            runCatching {
                check(!closed.get())
                val metrics = context.resources.displayMetrics
                width = 1280; height = 720
                mux = TsMux(width, height, internalAudio, sink)
                baseUs = System.nanoTime() / 1000
                if (permission != null) {
                val manager = context.getSystemService(MediaProjectionManager::class.java)
                projection = manager.getMediaProjection(Activity.RESULT_OK, permission)
                projection!!.registerCallback(object : MediaProjection.Callback() {
                    override fun onStop() { fail("CAPTURE_REVOKED") }
                    override fun onCapturedContentResize(w: Int, h: Int) {
                        // Never reuse an Android14 grant to create a second display.
                        if (w > 0 && h > 0 && kotlin.math.abs(w.toDouble() / h - width.toDouble() / height) > 0.1) fail("CAPTURE_SIZE_CHANGED_RESTART_REQUIRED")
                    }
                }, worker)
                }
                val format = MediaFormat.createVideoFormat(MediaFormat.MIMETYPE_VIDEO_AVC, width, height).apply {
                    setInteger(MediaFormat.KEY_COLOR_FORMAT, MediaCodecInfo.CodecCapabilities.COLOR_FormatSurface)
                    setInteger(MediaFormat.KEY_BIT_RATE, 3_000_000)
                    setInteger(MediaFormat.KEY_FRAME_RATE, 30)
                    setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, 1)
                    setInteger(MediaFormat.KEY_PROFILE, MediaCodecInfo.CodecProfileLevel.AVCProfileBaseline)
                    setInteger(MediaFormat.KEY_MAX_B_FRAMES, 0)
                }
                val codec = MediaCodecList(MediaCodecList.REGULAR_CODECS).codecInfos.firstOrNull {
                    it.isEncoder && it.isHardwareAccelerated && it.supportedTypes.any { t -> t.equals(MediaFormat.MIMETYPE_VIDEO_AVC, true) } && it.getCapabilitiesForType(MediaFormat.MIMETYPE_VIDEO_AVC).isFormatSupported(format)
                } ?: error("H264_HARDWARE_UNAVAILABLE")
                video = MediaCodec.createByCodecName(codec.name).also { encoder ->
                    encoder.setCallback(object : MediaCodec.Callback() {
                        override fun onInputBufferAvailable(codec: MediaCodec, index: Int) {}
                        override fun onOutputFormatChanged(codec: MediaCodec, format: MediaFormat) {
                            config = listOf("csd-0", "csd-1").flatMap { key ->
                                format.getByteBuffer(key)?.duplicate()?.let { b -> ByteArray(b.remaining()).also { b.get(it) }.toList() } ?: emptyList()
                            }.toByteArray()
                        }
                        override fun onError(codec: MediaCodec, error: MediaCodec.CodecException) { fail("H264_ENCODER_FAILED") }
                        override fun onOutputBufferAvailable(codec: MediaCodec, index: Int, info: MediaCodec.BufferInfo) {
                            try {
                                if (!closed.get() && info.size > 0 && info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG == 0) {
                                    val buffer = checkNotNull(codec.getOutputBuffer(index)).duplicate()
                                    buffer.position(info.offset); buffer.limit(info.offset + info.size)
                                    var bytes = ByteArray(info.size).also { buffer.get(it) }
                                    if (info.flags and MediaCodec.BUFFER_FLAG_KEY_FRAME != 0) bytes = config + bytes
                                    mux?.video(bytes, (info.presentationTimeUs - baseUs).coerceAtLeast(0))
                                }
                            } catch (_: Exception) { fail("TS_VIDEO_FAILED") }
                            finally { runCatching { codec.releaseOutputBuffer(index, false) } }
                        }
                    }, worker)
                    encoder.configure(format, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
                    surface = encoder.createInputSurface(); encoder.start()
                }
                if (permission != null) display = projection!!.createVirtualDisplay("LanCast", width, height, metrics.densityDpi, DisplayManager.VIRTUAL_DISPLAY_FLAG_AUTO_MIRROR, surface, null, worker)
                else {
                    probeFrames = ProbeFrames(checkNotNull(surface))
                    var frame = 0L
                    val draw = object : Runnable {
                        override fun run() {
                            if (closed.get()) return
                            try { probeFrames?.draw(frame++, System.nanoTime()); worker.postDelayed(this, 33) }
                            catch (_: Exception) { fail("PROBE_RENDER_FAILED") }
                        }
                    }
                    worker.post(draw)
                }
                if (internalAudio) startAudio(permission == null)
                status("dlna_capture_started")
            }.onFailure { fail(it.message ?: "CAPTURE_FAILED") }
        }
    }
    @Suppress("MissingPermission")
    private fun startAudio(synthetic: Boolean) {
        val recorder = if (!synthetic) {
        val capture = AudioPlaybackCaptureConfiguration.Builder(projection!!)
            .addMatchingUsage(AudioAttributes.USAGE_MEDIA).addMatchingUsage(AudioAttributes.USAGE_GAME).addMatchingUsage(AudioAttributes.USAGE_UNKNOWN).build()
        val format = AudioFormat.Builder().setEncoding(AudioFormat.ENCODING_PCM_16BIT).setSampleRate(48000).setChannelMask(AudioFormat.CHANNEL_IN_STEREO).build()
        val minimum = AudioRecord.getMinBufferSize(48000, AudioFormat.CHANNEL_IN_STEREO, AudioFormat.ENCODING_PCM_16BIT)
        check(minimum > 0)
        val recorder = AudioRecord.Builder().setAudioFormat(format).setBufferSizeInBytes(maxOf(minimum, 19200)).setAudioPlaybackCaptureConfig(capture).build()
        record = recorder
        check(recorder.state == AudioRecord.STATE_INITIALIZED) { "AUDIO_NOT_CAPTURABLE" }
        recorder
        } else null
        val encoder = MediaCodec.createEncoderByType(MediaFormat.MIMETYPE_AUDIO_AAC)
        audio = encoder
        encoder.configure(MediaFormat.createAudioFormat(MediaFormat.MIMETYPE_AUDIO_AAC, 48000, 2).apply {
            setInteger(MediaFormat.KEY_AAC_PROFILE, MediaCodecInfo.CodecProfileLevel.AACObjectLC)
            setInteger(MediaFormat.KEY_BIT_RATE, 128000)
            setInteger(MediaFormat.KEY_MAX_INPUT_SIZE, 4096)
        }, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
        encoder.start(); recorder?.startRecording()
        // This thread exclusively owns audio codec I/O. Mux calls run on the shared worker.
        audioThread = Thread({
            val pcm = ByteArray(4096); var frames = 0L
            val origin = System.nanoTime() / 1000 - baseUs
            val info = MediaCodec.BufferInfo()
            val pending = java.util.concurrent.Semaphore(2)
            try {
                while (!closed.get()) {
                    val index = encoder.dequeueInputBuffer(10000)
                    if (index >= 0) {
                        val count = if (recorder != null) recorder.read(pcm, 0, pcm.size, AudioRecord.READ_BLOCKING) else {
                            for (i in 0 until 1024) {
                                val sample = (kotlin.math.sin((frames + i) * 2.0 * Math.PI * 440 / 48000) * 4000).toInt()
                                pcm[i * 4] = sample.toByte(); pcm[i * 4 + 1] = (sample shr 8).toByte()
                                pcm[i * 4 + 2] = sample.toByte(); pcm[i * 4 + 3] = (sample shr 8).toByte()
                            }
                            val wait = (baseUs + origin + frames * 1_000_000 / 48000 - System.nanoTime() / 1000) / 1000
                            if (wait > 0) Thread.sleep(wait.coerceAtMost(30))
                            pcm.size
                        }
                        if (count <= 0) { if (!closed.get()) error("AUDIO_NOT_CAPTURABLE"); break }
                        val input = checkNotNull(encoder.getInputBuffer(index)); input.clear(); input.put(pcm, 0, count)
                        encoder.queueInputBuffer(index, 0, count, origin + frames * 1_000_000 / 48000, 0); frames += count / 4
                    }
                    var output = encoder.dequeueOutputBuffer(info, 0)
                    while (output >= 0) {
                        if (info.size > 0 && info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG == 0) {
                            val buffer = checkNotNull(encoder.getOutputBuffer(output)).duplicate(); buffer.position(info.offset); buffer.limit(info.offset + info.size)
                            val bytes = ByteArray(info.size).also { buffer.get(it) }; val pts = info.presentationTimeUs
                            check(pending.tryAcquire()) { "AUDIO_BACKPRESSURE" }
                            worker.post { try { if (!closed.get()) mux?.audio(bytes, pts) } catch (_: Exception) { fail("TS_AUDIO_FAILED") } finally { pending.release() } }
                        }
                        encoder.releaseOutputBuffer(output, false); output = encoder.dequeueOutputBuffer(info, 0)
                    }
                }
            } catch (e: Exception) { fail(e.message ?: "AUDIO_FAILED") }
        }, "lancast-playback-aac").apply { start() }
    }
    override fun close() {
        if (!closed.compareAndSet(false, true)) return
        // AudioRecord.stop unblocks the only reader before the worker waits for its exit.
        runCatching { record?.stop() }
        worker.post {
            audioThread?.join(2000)
            runCatching { probeFrames?.close() }
            runCatching { display?.release(); surface?.release(); projection?.stop() }
            runCatching { video?.stop(); video?.release() }
            if (audioThread?.isAlive != true) runCatching { audio?.stop(); audio?.release(); record?.release() }
            runCatching { mux?.close() }
            worker.removeCallbacksAndMessages(null); thread.quitSafely()
        }
    }
}
