package dev.lancast.airplay

import android.content.Context
import android.media.*
import android.os.Build
import android.view.Surface
import dev.lancast.receiver.contracts.MediaQueue
import java.nio.ByteBuffer
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.math.pow

/** Each worker exclusively owns its codec. Shutdown interrupts scheduling and joins both workers. */
internal class MediaPipeline(
    context: Context, private val surface: Surface,
    private val firstFrame: () -> Unit, private val failed: (String) -> Unit
) : AutoCloseable {
    private sealed interface VideoWork {
        data class Format(val width: Int, val height: Int, val sps: ByteArray, val pps: ByteArray) : VideoWork
        data class Frame(val pts: Long, val key: Boolean, val data: ByteArray) : VideoWork
    }
    private sealed interface AudioWork {
        data class Format(val codec: Int, val rate: Int, val channels: Int, val spf: Int) : AudioWork
        data class Frame(val pts: Long, val data: ByteArray) : AudioWork
    }
    private val audioManager = context.applicationContext.getSystemService(Context.AUDIO_SERVICE) as AudioManager
    private val running = AtomicBoolean(true)
    private val reported = AtomicBoolean(false)
    @Volatile private var volume = 1f
    fun setVolume(db: Float) { if (db.isFinite()) volume = if (db <= -144f) 0f else 10.0.pow(db.coerceIn(-30f, 0f) / 20.0).toFloat() }
    private val videoQueue = MediaQueue<VideoWork>(24, 8 * 1024 * 1024, 300_000)
    private val audioQueue = MediaQueue<AudioWork>(64, 1024 * 1024, 500_000)
    private val unixToMonotonicUs = System.nanoTime() / 1000 - System.currentTimeMillis() * 1000
    private val videoWorker = Thread(::videoLoop, "airplay-video").apply { start() }
    private val audioWorker = Thread(::audioLoop, "airplay-audio").apply { start() }

    fun videoConfig(w: Int, h: Int, sps: ByteArray, pps: ByteArray) = running.get() && videoQueue.offer(VideoWork.Format(w, h, sps, pps), sps.size + pps.size, 0)
    fun video(pts: Long, key: Boolean, data: ByteArray) = running.get() && videoQueue.offer(VideoWork.Frame(pts, key, data), data.size, pts)
    fun audioConfig(codec: Int, rate: Int, channels: Int, spf: Int) = running.get() && audioQueue.offer(AudioWork.Format(codec, rate, channels, spf), 0, 0)
    fun audio(pts: Long, data: ByteArray) = running.get() && audioQueue.offer(AudioWork.Frame(pts, data), data.size, pts)

    private fun error(message: String) {
        if (running.getAndSet(false)) {
            videoQueue.close(); audioQueue.close()
            failed(message)
        }
    }
    // A common playout margin lets both decoders schedule against the sender's clock.
    private fun monotonic(pts: Long) = pts + unixToMonotonicUs + 180_000
    private fun waitUntil(pts: Long) {
        val target = monotonic(pts)
        while (running.get()) {
            val remaining = target - System.nanoTime() / 1000
            if (remaining <= 0) return
            if (remaining > 2_000_000) throw IllegalStateException("播放时钟超出范围")
            Thread.sleep((remaining / 1000).coerceIn(1, 10))
        }
    }
    private fun videoLoop() {
        var codec: MediaCodec? = null
        var needKey = true
        val info = MediaCodec.BufferInfo()
        try {
            while (running.get()) {
                val item = if (codec == null) videoQueue.take() else videoQueue.poll(DECODER_POLL_MS)
                if (!running.get()) break
                when (item) {
                    is VideoWork.Format -> {
                        releaseDecoder(codec); codec = null
                        val format = MediaFormat.createVideoFormat("video/avc", item.width, item.height)
                        format.setByteBuffer("csd-0", ByteBuffer.wrap(byteArrayOf(0, 0, 0, 1) + item.sps))
                        format.setByteBuffer("csd-1", ByteBuffer.wrap(byteArrayOf(0, 0, 0, 1) + item.pps))
                        format.setInteger(MediaFormat.KEY_MAX_INPUT_SIZE, 2 * 1024 * 1024)
                        if (Build.VERSION.SDK_INT >= 30) format.setInteger(MediaFormat.KEY_LOW_LATENCY, 1)
                        val name = MediaCodecList(MediaCodecList.REGULAR_CODECS).findDecoderForFormat(format)
                            ?: throw IllegalStateException("电视不支持此 H.264 格式")
                        codec = MediaCodec.createByCodecName(name).configureOrRelease({ it.release() }) {
                            it.configure(format, surface, null, 0); it.start()
                            it.setOnFrameRenderedListener({ _, _, _ -> if (running.get() && reported.compareAndSet(false, true)) firstFrame() }, android.os.Handler(android.os.Looper.getMainLooper()))
                        }
                        needKey = true
                    }
                    is VideoWork.Frame -> {
                        val active = codec ?: continue
                        if (needKey && !item.key) continue
                        needKey = false
                        val input = active.dequeueInputBuffer(10_000)
                        if (input < 0) throw IllegalStateException("视频解码器处理不及时")
                        val buffer = active.getInputBuffer(input)!!
                        require(item.data.size <= buffer.capacity()) { "视频帧超过解码器容量" }
                        buffer.clear(); buffer.put(item.data)
                        active.queueInputBuffer(input, 0, item.data.size, item.pts, 0)
                    }
                    null -> Unit
                }
                val active = codec ?: continue
                var output = active.dequeueOutputBuffer(info, 0)
                while (output >= 0) {
                    val target = monotonic(info.presentationTimeUs)
                    val delta = target - System.nanoTime() / 1000
                    if (delta < -500_000) active.releaseOutputBuffer(output, false)
                    else {
                        if (delta > 2_000_000) throw IllegalStateException("视频时钟超出范围")
                        active.releaseOutputBuffer(output, target.coerceAtLeast(System.nanoTime() / 1000) * 1000)
                    }
                    output = active.dequeueOutputBuffer(info, 0)
                }
            }
        } catch (_: InterruptedException) { /* explicit stop */ }
        catch (e: Exception) { error(e.message ?: "视频播放失败") }
        finally { releaseDecoder(codec); videoQueue.close() }
    }

    @Suppress("DEPRECATION")
    private fun audioLoop() {
        var codec: MediaCodec? = null
        var track: AudioTrack? = null
        var format: AudioWork.Format? = null
        var focus: AudioFocusRequest? = null
        var hasFocus = false
        val focusListener = AudioManager.OnAudioFocusChangeListener { if (it == AudioManager.AUDIOFOCUS_LOSS || it == AudioManager.AUDIOFOCUS_LOSS_TRANSIENT) error("声音播放被其他应用中断") }
        val info = MediaCodec.BufferInfo()
        val audioTime = AudioTimestamp()
        var writtenFrames = 0L
        var outputRate = 44100
        var outputChannels = 2
        var appliedVolume = -1f
        fun createTrack(rate: Int, channels: Int): AudioTrack {
            writtenFrames = 0; outputRate = rate; outputChannels = channels
            val mask = if (channels == 1) AudioFormat.CHANNEL_OUT_MONO else AudioFormat.CHANNEL_OUT_STEREO
            val size = AudioTrack.getMinBufferSize(rate, mask, AudioFormat.ENCODING_PCM_16BIT)
            require(size > 0) { "电视不支持此声音输出格式" }
            return AudioTrack.Builder().setAudioAttributes(AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_MEDIA).setContentType(AudioAttributes.CONTENT_TYPE_MOVIE).build())
                .setAudioFormat(AudioFormat.Builder().setEncoding(AudioFormat.ENCODING_PCM_16BIT).setSampleRate(rate).setChannelMask(mask).build())
                .setTransferMode(AudioTrack.MODE_STREAM).setBufferSizeInBytes(size * 2).build().configureOrRelease({ it.release() }) {
                    check(it.state == AudioTrack.STATE_INITIALIZED)
                    it.setVolume(volume); it.play(); appliedVolume = volume
                }
        }
        fun writePcm(bytes: ByteBuffer, pts: Long) {
            val active = track ?: return
            if (appliedVolume != volume) { active.setVolume(volume); appliedVolume = volume }
            // Use AudioTrack's actual playback clock to account for queued samples.
            if (active.getTimestamp(audioTime)) {
                val now = System.nanoTime() / 1000
                val nextPlay = (audioTime.nanoTime / 1000 + (writtenFrames - audioTime.framePosition).coerceAtLeast(0) * 1_000_000 / outputRate).coerceAtLeast(now)
                val correction = monotonic(pts) - nextPlay
                if (correction < -100_000) return
                require(correction <= 2_000_000) { "声音时钟超出范围" }
                val writeAt = now + correction.coerceAtLeast(0)
                while (running.get() && System.nanoTime() / 1000 < writeAt) Thread.sleep(2)
            } else waitUntil(pts)
            val initial = bytes.remaining()
            while (running.get() && bytes.hasRemaining()) {
                val n = active.write(bytes, bytes.remaining(), AudioTrack.WRITE_NON_BLOCKING)
                if (n < 0) throw IllegalStateException("声音输出失败")
                if (n == 0) Thread.sleep(2)
            }
            writtenFrames += (initial - bytes.remaining()) / (2 * outputChannels)
        }
        try {
            while (running.get()) {
                val item = if (codec == null) audioQueue.take() else audioQueue.poll(DECODER_POLL_MS)
                if (!running.get()) break
                when (item) {
                    is AudioWork.Format -> {
                        releaseDecoder(codec); codec = null
                        releaseTrack(track); track = null
                        if (!hasFocus) {
                            val focusResult = if (Build.VERSION.SDK_INT >= 26) {
                                focus = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN).setOnAudioFocusChangeListener(focusListener)
                                    .setAudioAttributes(AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_MEDIA).build()).build()
                                audioManager.requestAudioFocus(focus!!)
                            } else audioManager.requestAudioFocus(focusListener, AudioManager.STREAM_MUSIC, AudioManager.AUDIOFOCUS_GAIN)
                            check(focusResult == AudioManager.AUDIOFOCUS_REQUEST_GRANTED) { "无法获得声音播放权限" }
                            hasFocus = true
                        }
                        format = item
                        if (item.codec == 3) track = createTrack(item.rate, item.channels)
                        else {
                            val mf = aacFormat(item.codec, item.rate, item.channels, item.spf)
                            val name = MediaCodecList(MediaCodecList.REGULAR_CODECS).findDecoderForFormat(mf) ?: throw IllegalStateException("电视不支持此 AAC 声音格式")
                            codec = MediaCodec.createByCodecName(name).configureOrRelease({ it.release() }) { it.configure(mf, null, null, 0); it.start() }
                        }
                    }
                    is AudioWork.Frame -> {
                        if (format?.codec == 3) writePcm(ByteBuffer.wrap(item.data), item.pts)
                        else {
                            val active = codec ?: continue
                            val input = active.dequeueInputBuffer(10_000)
                            if (input < 0) throw IllegalStateException("声音解码器处理不及时")
                            val buffer = active.getInputBuffer(input)!!; require(item.data.size <= buffer.capacity())
                            buffer.clear(); buffer.put(item.data); active.queueInputBuffer(input, 0, item.data.size, item.pts, 0)
                        }
                    }
                    null -> Unit
                }
                val active = codec ?: continue
                var output = active.dequeueOutputBuffer(info, 0)
                if (output == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED) {
                    val decoded = active.outputFormat
                    if (Build.VERSION.SDK_INT >= 24 && decoded.containsKey(MediaFormat.KEY_PCM_ENCODING)) check(decoded.getInteger(MediaFormat.KEY_PCM_ENCODING) == AudioFormat.ENCODING_PCM_16BIT)
                    releaseTrack(track); track = null
                    track = createTrack(decoded.getInteger(MediaFormat.KEY_SAMPLE_RATE), decoded.getInteger(MediaFormat.KEY_CHANNEL_COUNT))
                    output = active.dequeueOutputBuffer(info, 0)
                }
                while (output >= 0) {
                    try {
                        if (info.size > 0) {
                            val buffer = active.getOutputBuffer(output)!!
                            buffer.limit(info.offset + info.size); buffer.position(info.offset)
                            // AudioTrack advances this buffer directly; keep it owned until the write ends.
                            writePcm(buffer, info.presentationTimeUs)
                        }
                    } finally { active.releaseOutputBuffer(output, false) }
                    output = active.dequeueOutputBuffer(info, 0)
                }
            }
        } catch (_: InterruptedException) { /* explicit stop */ }
        catch (e: Exception) { error(e.message ?: "声音播放失败") }
        finally {
            releaseDecoder(codec)
            releaseTrack(track)
            if (hasFocus) runCatching {
                if (Build.VERSION.SDK_INT >= 26 && focus != null) audioManager.abandonAudioFocusRequest(focus!!) else audioManager.abandonAudioFocus(focusListener)
            }
            audioQueue.close()
        }
    }
    override fun close() {
        running.set(false); videoQueue.close(); audioQueue.close()
        videoWorker.interrupt(); audioWorker.interrupt()
        videoWorker.join(); audioWorker.join()
    }
    companion object {
        private const val DECODER_POLL_MS = 10L
        private fun releaseDecoder(codec: MediaCodec?) {
            codec?.let { runCatching { it.stop() }; runCatching { it.release() } }
        }
        private fun releaseTrack(track: AudioTrack?) {
            track?.let { runCatching { it.pause() }; runCatching { it.flush() }; runCatching { it.release() } }
        }
        internal fun aacFormat(codec: Int, rate: Int, channels: Int, spf: Int): MediaFormat {
            require(codec in 1..2 && rate in listOf(44100, 48000) && channels == 2)
            val profile = if (codec == 2) MediaCodecInfo.CodecProfileLevel.AACObjectELD else MediaCodecInfo.CodecProfileLevel.AACObjectLC
            val bits = StringBuilder()
            fun put(value: Int, count: Int) { for (i in count - 1 downTo 0) bits.append(if (value and (1 shl i) != 0) '1' else '0') }
            if (profile >= 32) { put(31, 5); put(profile - 32, 6) } else put(profile, 5)
            put(if (rate == 44100) 4 else 3, 4); put(channels, 4)
            if (codec == 2) { put(if (spf == 480) 1 else 0, 1); put(0, 10) } else put(0, 3)
            while (bits.length % 8 != 0) bits.append('0')
            val config = bits.chunked(8).map { it.toInt(2).toByte() }.toByteArray()
            return MediaFormat.createAudioFormat("audio/mp4a-latm", rate, channels).apply {
                setInteger(MediaFormat.KEY_AAC_PROFILE, profile); setInteger(MediaFormat.KEY_MAX_INPUT_SIZE, 16384)
                setByteBuffer("csd-0", ByteBuffer.wrap(config))
            }
        }
        fun checkCapabilities() {
            val codecs = MediaCodecList(MediaCodecList.REGULAR_CODECS)
            check(codecs.findDecoderForFormat(MediaFormat.createVideoFormat("video/avc", 1920, 1080)) != null) { "电视缺少 H.264 解码器" }
            for (rate in listOf(44100, 48000)) {
                check(codecs.findDecoderForFormat(aacFormat(2, rate, 2, 480)) != null) { "电视缺少 AAC-ELD 解码器，暂不能接收苹果镜像声音" }
                check(codecs.findDecoderForFormat(aacFormat(1, rate, 2, 1024)) != null) { "电视缺少 AAC-LC 解码器" }
            }
        }
    }
}
