package dev.lancast.media

import android.annotation.SuppressLint
import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioPlaybackCaptureConfiguration
import android.media.AudioRecord
import android.media.AudioTimestamp
import android.media.projection.MediaProjection
import android.os.Build
import androidx.annotation.RequiresApi
import org.webrtc.audio.JavaAudioDeviceModule
import java.nio.ByteBuffer
import java.util.concurrent.atomic.AtomicBoolean

/** No microphone is opened. The WebRTC ADM must have setAudioRecordEnabled(false). */
@RequiresApi(Build.VERSION_CODES.Q)
class PlaybackAudio(private val onError: (String) -> Unit) : JavaAudioDeviceModule.AudioBufferCallback {
    @Volatile private var record: AudioRecord? = null
    private val closed = AtomicBoolean(false)
    private val timestamp = AudioTimestamp()
    private var framesRead = 0L
    private var silentBuffers = 0
    private var warned = false
    @SuppressLint("MissingPermission")
    fun start(projection: MediaProjection) {
        check(!closed.get() && record == null)
        val configuration = AudioPlaybackCaptureConfiguration.Builder(projection)
            .addMatchingUsage(AudioAttributes.USAGE_MEDIA)
            .addMatchingUsage(AudioAttributes.USAGE_GAME)
            .addMatchingUsage(AudioAttributes.USAGE_UNKNOWN).build()
        val audio = AudioRecord.Builder().setAudioPlaybackCaptureConfig(configuration)
            .setAudioFormat(AudioFormat.Builder().setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                .setSampleRate(48000).setChannelMask(AudioFormat.CHANNEL_IN_MONO).build())
            .setBufferSizeInBytes(maxOf(9600, AudioRecord.getMinBufferSize(48000, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT))).build()
        check(audio.state == AudioRecord.STATE_INITIALIZED) { "AUDIO_NOT_CAPTURABLE" }
        record = audio
        audio.startRecording()
    }
    override fun onBuffer(buffer: ByteBuffer, format: Int, channels: Int, rate: Int, bytesRead: Int, captureTimeNs: Long): Long {
        val audio = record
        if (closed.get() || audio == null) {
            buffer.clear(); while (buffer.hasRemaining()) buffer.put(0)
            Thread.sleep(10)
            return System.nanoTime()
        }
        require(rate == 48000 && channels == 1 && format == AudioFormat.ENCODING_PCM_16BIT)
        buffer.clear()
        val count = audio.read(buffer, buffer.capacity(), AudioRecord.READ_BLOCKING)
        if (count <= 0) {
            if (!closed.get()) onError("AUDIO_NOT_CAPTURABLE")
            buffer.clear(); while (buffer.hasRemaining()) buffer.put(0)
            Thread.sleep(10)
            return System.nanoTime()
        }
        val firstFrame = framesRead
        framesRead += count / 2
        var anySound = false
        for (i in 0 until count) if (buffer.get(i).toInt() != 0) { anySound = true; break }
        silentBuffers = if (anySound) 0 else silentBuffers + 1
        if (silentBuffers >= 500 && !warned) {
            warned = true
            onError("持续未检测到内部声音：来源可能静音或禁止采集")
        }
        for (i in count until buffer.capacity()) buffer.put(i, 0)
        return if (audio.getTimestamp(timestamp, AudioTimestamp.TIMEBASE_MONOTONIC) == AudioRecord.SUCCESS)
            timestamp.nanoTime + (firstFrame - timestamp.framePosition) * 1_000_000_000L / rate
        else 0L
    }
    /** Stop unblocks READ_BLOCKING; release only after WebRTC's callback thread has joined. */
    fun stop() { closed.set(true); runCatching { record?.stop() } }
    fun release() { stop(); record?.release(); record = null }
}
