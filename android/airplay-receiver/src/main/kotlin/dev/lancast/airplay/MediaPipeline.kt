package dev.lancast.airplay

import android.content.Context
import android.media.*
import android.os.Build
import android.view.Surface
import dev.lancast.receiver.contracts.MediaQueue
import java.nio.ByteBuffer
import java.util.concurrent.atomic.AtomicBoolean

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
    private val videoQueue = MediaQueue<VideoWork>(24, 8 * 1024 * 1024, 300_000)
    private val audioQueue = MediaQueue<AudioWork>(64, 1024 * 1024, 500_000)
    private val unixToMonotonicUs = System.nanoTime() / 1000 - System.currentTimeMillis() * 1000
    private val videoWorker = Thread(::videoLoop, "airplay-video").apply { start() }
    private val audioWorker = Thread(::audioLoop, "airplay-audio").apply { start() }

    fun videoConfig(w: Int, h: Int, sps: ByteArray, pps: ByteArray) = running.get() && videoQueue.offer(VideoWork.Format(w, h, sps, pps), sps.size + pps.size, 0)
    fun video(pts: Long, key: Boolean, data: ByteArray) = running.get() && videoQueue.offer(VideoWork.Frame(pts, key, data), data.size, pts)
    fun audioConfig(codec: Int, rate: Int, channels: Int, spf: Int) = running.get() && audioQueue.offer(AudioWork.Format(codec, rate, channels, spf), 0, 0)
    fun audio(pts: Long, data: ByteArray) = running.get() && audioQueue.offer(AudioWork.Frame(pts, data), data.size, pts)

    private fun error(message: String) { if (running.getAndSet(false)) failed(message) }
    private fun monotonic(pts: Long) = pts + unixToMonotonicUs
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
                when (val item = videoQueue.poll()) {
                    is VideoWork.Format -> {
                        codec?.let { it.stop(); it.release() }; codec = null
                        val format = MediaFormat.createVideoFormat("video/avc", item.width, item.height)
                        format.setByteBuffer("csd-0", ByteBuffer.wrap(byteArrayOf(0, 0, 0, 1) + item.sps))
                        format.setByteBuffer("csd-1", ByteBuffer.wrap(byteArrayOf(0, 0, 0, 1) + item.pps))
                        format.setInteger(MediaFormat.KEY_MAX_INPUT_SIZE, 2 * 1024 * 1024)
                        if (Build.VERSION.SDK_INT >= 30) format.setInteger(MediaFormat.KEY_LOW_LATENCY, 1)
                        val name = MediaCodecList(MediaCodecList.REGULAR_CODECS).findDecoderForFormat(format)
                            ?: throw IllegalStateException("电视不支持此 H.264 格式")
                        codec = MediaCodec.createByCodecName(name).also { it.configure(format, surface, null, 0); it.start() }
                        codec.setOnFrameRenderedListener({ _, _, _ -> if (running.get() && reported.compareAndSet(false, true)) firstFrame() }, android.os.Handler(android.os.Looper.getMainLooper()))
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
                    null -> Thread.sleep(2)
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
        finally { codec?.let { runCatching { it.stop() }; runCatching { it.release() } }; videoQueue.clear() }
    }

    @Suppress("DEPRECATION")
    private fun audioLoop() {
        var codec: MediaCodec? = null
        var track: AudioTrack? = null
        var format: AudioWork.Format? = null
        var focus: AudioFocusRequest? = null
        val focusListener = AudioManager.OnAudioFocusChangeListener { if (it == AudioManager.AUDIOFOCUS_LOSS || it == AudioManager.AUDIOFOCUS_LOSS_TRANSIENT) error("声音播放被其他应用中断") }
        val info = MediaCodec.BufferInfo()
        fun createTrack(rate: Int, channels: Int): AudioTrack {
            val mask = if (channels == 1) AudioFormat.CHANNEL_OUT_MONO else AudioFormat.CHANNEL_OUT_STEREO
            val size = AudioTrack.getMinBufferSize(rate, mask, AudioFormat.ENCODING_PCM_16BIT)
            require(size > 0) { "电视不支持此声音输出格式" }
            return AudioTrack.Builder().setAudioAttributes(AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_MEDIA).setContentType(AudioAttributes.CONTENT_TYPE_MOVIE).build())
                .setAudioFormat(AudioFormat.Builder().setEncoding(AudioFormat.ENCODING_PCM_16BIT).setSampleRate(rate).setChannelMask(mask).build())
                .setTransferMode(AudioTrack.MODE_STREAM).setBufferSizeInBytes(size * 2).build().also { check(it.state == AudioTrack.STATE_INITIALIZED); it.play() }
        }
        fun writePcm(bytes: ByteArray, pts: Long) {
            val active = track ?: return
            // Schedule against the protocol clock, compensating already queued audio.
            val audioTime = AudioTimestamp()
            if (active.getTimestamp(audioTime)) {
                // AudioTrack maintains its own sample clock once started. The queue remains bounded.
                if (monotonic(pts) - System.nanoTime() / 1000 < -500_000) return
            }
            waitUntil(pts)
            var offset = 0
            while (running.get() && offset < bytes.size) {
                val n = active.write(bytes, offset, bytes.size - offset, AudioTrack.WRITE_NON_BLOCKING)
                if (n < 0) throw IllegalStateException("声音输出失败")
                if (n == 0) Thread.sleep(2) else offset += n
            }
        }
        try {
            while (running.get()) {
                when (val item = audioQueue.poll()) {
                    is AudioWork.Format -> {
                        codec?.let { it.stop(); it.release() }; codec = null
                        track?.let { it.pause(); it.flush(); it.release() }; track = null
                        val focusResult = if (Build.VERSION.SDK_INT >= 26) {
                            focus = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN).setOnAudioFocusChangeListener(focusListener)
                                .setAudioAttributes(AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_MEDIA).build()).build()
                            audioManager.requestAudioFocus(focus!!)
                        } else audioManager.requestAudioFocus(focusListener, AudioManager.STREAM_MUSIC, AudioManager.AUDIOFOCUS_GAIN)
                        check(focusResult == AudioManager.AUDIOFOCUS_REQUEST_GRANTED) { "无法获得声音播放权限" }
                        format = item
                        if (item.codec == 3) track = createTrack(item.rate, item.channels)
                        else {
                            val mf = aacFormat(item.codec, item.rate, item.channels, item.spf)
                            val name = MediaCodecList(MediaCodecList.REGULAR_CODECS).findDecoderForFormat(mf) ?: throw IllegalStateException("电视不支持此 AAC 声音格式")
                            codec = MediaCodec.createByCodecName(name).also { it.configure(mf, null, null, 0); it.start() }
                        }
                    }
                    is AudioWork.Frame -> {
                        if (format?.codec == 3) writePcm(item.data, item.pts)
                        else {
                            val active = codec ?: continue
                            val input = active.dequeueInputBuffer(10_000)
                            if (input < 0) throw IllegalStateException("声音解码器处理不及时")
                            val buffer = active.getInputBuffer(input)!!; require(item.data.size <= buffer.capacity())
                            buffer.clear(); buffer.put(item.data); active.queueInputBuffer(input, 0, item.data.size, item.pts, 0)
                        }
                    }
                    null -> Thread.sleep(2)
                }
                val active = codec ?: continue
                var output = active.dequeueOutputBuffer(info, 0)
                if (output == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED) {
                    val decoded = active.outputFormat
                    if (Build.VERSION.SDK_INT >= 24 && decoded.containsKey(MediaFormat.KEY_PCM_ENCODING)) check(decoded.getInteger(MediaFormat.KEY_PCM_ENCODING) == AudioFormat.ENCODING_PCM_16BIT)
                    track?.release(); track = createTrack(decoded.getInteger(MediaFormat.KEY_SAMPLE_RATE), decoded.getInteger(MediaFormat.KEY_CHANNEL_COUNT))
                    output = active.dequeueOutputBuffer(info, 0)
                }
                while (output >= 0) {
                    val buffer = active.getOutputBuffer(output)!!
                    buffer.position(info.offset); buffer.limit(info.offset + info.size)
                    val bytes = ByteArray(info.size); buffer.get(bytes)
                    active.releaseOutputBuffer(output, false); writePcm(bytes, info.presentationTimeUs)
                    output = active.dequeueOutputBuffer(info, 0)
                }
            }
        } catch (_: InterruptedException) { /* explicit stop */ }
        catch (e: Exception) { error(e.message ?: "声音播放失败") }
        finally {
            codec?.let { runCatching { it.stop() }; runCatching { it.release() } }
            track?.let { runCatching { it.pause() }; runCatching { it.flush() }; runCatching { it.release() } }
            if (Build.VERSION.SDK_INT >= 26 && focus != null) audioManager.abandonAudioFocusRequest(focus!!) else audioManager.abandonAudioFocus(focusListener)
            audioQueue.clear()
        }
    }
    override fun close() {
        running.set(false); videoWorker.interrupt(); audioWorker.interrupt()
        videoWorker.join(); audioWorker.join()
    }
    companion object {
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
