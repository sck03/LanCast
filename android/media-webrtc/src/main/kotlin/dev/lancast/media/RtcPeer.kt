package dev.lancast.media

import android.content.Context
import android.content.Intent
import android.media.AudioAttributes
import android.media.projection.MediaProjection
import android.os.Build
import android.os.Handler
import android.os.HandlerThread
import org.json.JSONObject
import org.webrtc.*
import org.webrtc.audio.JavaAudioDeviceModule
import java.io.Closeable
import java.util.UUID
import java.util.concurrent.atomic.AtomicBoolean

/** All PeerConnection mutations and disposal are serialized away from library callbacks. */
class RtcPeer(
    private val context: Context,
    private val renderer: SurfaceViewRenderer?,
    private val signal: (String, JSONObject) -> Unit,
    private val status: (String) -> Unit,
    private val recoveryEnabled: Boolean = false
) : Closeable {
    private val closed = AtomicBoolean(false)
    private val thread = HandlerThread("lancast-media").apply { start() }
    private val worker = Handler(thread.looper)
    private val egl = EglBase.create()
    private var audio: JavaAudioDeviceModule? = null
    private var factory: PeerConnectionFactory? = null
    private var peer: PeerConnection? = null
    private var videoSource: VideoSource? = null
    private var videoTrack: VideoTrack? = null
    private var audioSource: AudioSource? = null
    private var audioTrack: AudioTrack? = null
    private var capture: ScreenCapturerAndroid? = null
    private var texture: SurfaceTextureHelper? = null
    private var playback: PlaybackAudio? = null
    private var remoteVideo: VideoTrack? = null
    private var frameProbe: VideoSink? = null
    private var receivedFrame = false
    private var remoteSet = false
    private var negotiation = ""
    private val pendingIce = ArrayList<IceCandidate>()
    private val localIce = ArrayList<IceCandidate>()
    private var localDescriptionSent = false
    private var transportGeneration = 0L
    private var internalAudioEnabled = false
    private var senderProfile = JSONObject()
    init {
        PeerConnectionFactory.initialize(PeerConnectionFactory.InitializationOptions.builder(context.applicationContext).createInitializationOptions())
        renderer?.init(egl.eglBaseContext, object : RendererCommon.RendererEvents {
            override fun onFirstFrameRendered() {}
            override fun onFrameResolutionChanged(width: Int, height: Int, rotation: Int) {}
        })
        renderer?.setScalingType(RendererCommon.ScalingType.SCALE_ASPECT_FIT)
        renderer?.setEnableHardwareScaler(true)
    }
    private fun submit(action: () -> Unit) {
        if (!closed.get()) worker.post {
            if (!closed.get()) try { action() } catch (_: Exception) { status("MEDIA_FAILED"); close() }
        }
    }
    private fun initialize(internalAudio: Boolean) {
        check(peer == null)
        internalAudioEnabled = internalAudio
        val builder = JavaAudioDeviceModule.builder(context)
            .setSampleRate(48000).setUseStereoInput(false)
            .setUseHardwareAcousticEchoCanceler(false).setUseHardwareNoiseSuppressor(false)
            .setAudioAttributes(AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_MEDIA).setContentType(AudioAttributes.CONTENT_TYPE_MOVIE).build())
        if (internalAudio) {
            check(Build.VERSION.SDK_INT >= 29)
            playback = PlaybackAudio { status(it) }
            builder.setAudioBufferCallback(playback)
        }
        audio = builder.createAudioDeviceModule().apply { setAudioRecordEnabled(false) }
        factory = PeerConnectionFactory.builder().setAudioDeviceModule(audio)
            .setVideoEncoderFactory(HardwareVideoEncoderFactory(egl.eglBaseContext, false, true))
            .setVideoDecoderFactory(HardwareVideoDecoderFactory(egl.eglBaseContext)).createPeerConnectionFactory()
        createPeer()
        worker.postDelayed(stats, 1000)
    }
    private fun createPeer() {
        transportGeneration++
        remoteSet = false; localDescriptionSent = false
        receivedFrame = false
        pendingIce.clear(); localIce.clear()
        val config = PeerConnection.RTCConfiguration(emptyList()).apply {
            sdpSemantics = PeerConnection.SdpSemantics.UNIFIED_PLAN
            bundlePolicy = PeerConnection.BundlePolicy.MAXBUNDLE
        }
        peer = checkNotNull(factory!!.createPeerConnection(config, observer(transportGeneration)))
        peer!!.setAudioRecording(internalAudioEnabled)
        peer!!.setAudioPlayout(renderer != null)
        val epoch = transportGeneration
        worker.postDelayed({
            if (!closed.get() && epoch == transportGeneration && renderer != null && !receivedFrame) {
                status("RTC_FIRST_FRAME_FAILED"); close()
            }
        }, 20_000)
    }
    private fun replacePeer() {
        // Keep MediaProjection, its one-use grant, texture/source and audio input alive.
        transportGeneration++
        renderer?.let { remoteVideo?.removeSink(it) }; frameProbe?.let { remoteVideo?.removeSink(it) }
        remoteVideo = null; frameProbe = null
        peer?.close(); peer?.dispose(); peer = null
        createPeer()
    }
    fun receiveOffer(sdp: String, negotiationId: String) = submit {
        if (peer == null) initialize(false)
        else if (negotiation.isNotEmpty()) {
            if (negotiation == negotiationId) return@submit
            check(recoveryEnabled)
            replacePeer()
        }
        negotiation = negotiationId
        remoteSet = false
        peer!!.setRemoteDescription(sdpObserver(onSet = {
            remoteSet = true; flushIce()
            peer!!.createAnswer(sdpObserver(onCreate = { answer -> publishDescription(answer, "rtc.answer") }), MediaConstraints())
        }), SessionDescription(SessionDescription.Type.OFFER, sdp))
    }
    fun startCapture(grant: Intent, internalAudio: Boolean, profile: JSONObject) = submit {
        initialize(internalAudio)
        senderProfile = profile
        negotiation = UUID.randomUUID().toString()
        val f = factory!!
        videoSource = f.createVideoSource(true)
        texture = SurfaceTextureHelper.create("lancast-texture", egl.eglBaseContext)
        capture = ScreenCapturerAndroid(grant, object : MediaProjection.Callback() {
            override fun onStop() { if (!closed.get()) { status("CAPTURE_REVOKED"); close() } }
            override fun onCapturedContentResize(width: Int, height: Int) {
                if (Build.VERSION.SDK_INT >= 34 && width > 0 && height > 0)
                    submit { capture?.changeCaptureFormat(width, height, profile.optInt("fps", 30)) }
            }
        })
        capture!!.initialize(texture, context, videoSource!!.capturerObserver)
        capture!!.startCapture(profile.optInt("width", 1280), profile.optInt("height", 720), profile.optInt("fps", 30))
        if (internalAudio) playback!!.start(checkNotNull(capture!!.mediaProjection) { "CAPTURE_PROJECTION_UNAVAILABLE" })
        videoTrack = f.createVideoTrack("screen", videoSource)
        peer!!.addTrack(videoTrack, listOf("lancast"))
        if (internalAudio) {
            val constraints = MediaConstraints().apply {
                mandatory.add(MediaConstraints.KeyValuePair("googEchoCancellation", "false"))
                mandatory.add(MediaConstraints.KeyValuePair("googAutoGainControl", "false"))
                mandatory.add(MediaConstraints.KeyValuePair("googNoiseSuppression", "false"))
            }
            audioSource = f.createAudioSource(constraints)
            audioTrack = f.createAudioTrack("system-audio", audioSource)
            peer!!.addTrack(audioTrack, listOf("lancast"))
        }
        configureSender()
        peer!!.createOffer(sdpObserver(onCreate = { publishDescription(it, "rtc.offer") }), MediaConstraints())
    }
    private fun configureSender() {
        val codecs = factory!!.getRtpSenderCapabilities(MediaStreamTrack.MediaType.MEDIA_TYPE_VIDEO).codecs.filter { it.name.equals("H264", true) }
        check(codecs.isNotEmpty()) { "H264_ENCODER_UNAVAILABLE" }
        peer!!.transceivers.filter { it.mediaType == MediaStreamTrack.MediaType.MEDIA_TYPE_VIDEO }.forEach { it.setCodecPreferences(codecs) }
        peer!!.senders.filter { it.track()?.kind() == "video" }.forEach { sender ->
            val parameters = sender.parameters
            parameters.encodings.forEach { it.maxBitrateBps = senderProfile.optInt("bitrate", 3_000_000); it.maxFramerate = senderProfile.optInt("fps", 30) }
            check(sender.setParameters(parameters)) { "ENCODER_PARAMETER_FAILED" }
        }
    }
    fun restart(previous: String) = submit {
        if (!recoveryEnabled || previous != negotiation || videoTrack == null) return@submit
        replacePeer()
        negotiation = UUID.randomUUID().toString()
        peer!!.addTrack(videoTrack, listOf("lancast"))
        audioTrack?.let { peer!!.addTrack(it, listOf("lancast")) }
        configureSender()
        peer!!.createOffer(sdpObserver(onCreate = { publishDescription(it, "rtc.offer", previous) }), MediaConstraints())
        status("rtc_reconnecting")
    }
    fun receiveAnswer(sdp: String, negotiationId: String) = submit {
        if (negotiation != negotiationId) return@submit
        peer!!.setRemoteDescription(sdpObserver(onSet = { remoteSet = true; flushIce() }), SessionDescription(SessionDescription.Type.ANSWER, sdp))
    }
    fun ice(body: JSONObject) = submit {
        if (body.getString("negotiationId") != negotiation) return@submit
        val ice = IceCandidate(body.optString("sdpMid", "0"), body.getInt("sdpMLineIndex"), body.getString("candidate"))
        if (remoteSet) check(peer!!.addIceCandidate(ice)) else { check(pendingIce.size < 128); pendingIce.add(ice) }
    }
    private fun flushIce() { pendingIce.forEach { check(peer!!.addIceCandidate(it)) }; pendingIce.clear() }
    private fun publishDescription(description: SessionDescription, type: String, previous: String? = null) {
        peer!!.setLocalDescription(sdpObserver(onSet = {
            val body = JSONObject().put("sdp", description.description).put("negotiationId", negotiation)
            if (previous != null) body.put("previousNegotiationId", previous)
            signal(type, body)
            localDescriptionSent = true
            localIce.forEach { sendIce(it) }; localIce.clear()
        }), description)
    }
    private fun sendIce(ice: IceCandidate) { signal("rtc.ice", JSONObject().put("candidate", ice.sdp).put("sdpMid", ice.sdpMid).put("sdpMLineIndex", ice.sdpMLineIndex).put("negotiationId", negotiation)) }
    private fun sdpObserver(onCreate: (SessionDescription) -> Unit = {}, onSet: () -> Unit = {}, expected: String = negotiation) = object : SdpObserver {
        override fun onCreateSuccess(sdp: SessionDescription) { submit { if (expected == negotiation) onCreate(sdp) } }
        override fun onSetSuccess() { submit { if (expected == negotiation) onSet() } }
        override fun onCreateFailure(error: String) { submit { if (expected == negotiation) { status("SDP_FAILED"); close() } } }
        override fun onSetFailure(error: String) { submit { if (expected == negotiation) { status("SDP_FAILED"); close() } } }
    }
    private fun observer(epoch: Long) = object : PeerConnection.Observer {
        override fun onIceCandidate(candidate: IceCandidate) { submit { if (epoch != transportGeneration) return@submit; if (localDescriptionSent) sendIce(candidate) else { check(localIce.size < 128); localIce.add(candidate) } } }
        override fun onTrack(transceiver: RtpTransceiver) { submit {
            if (epoch != transportGeneration) return@submit
            val track = transceiver.receiver.track()
            if (track is VideoTrack) {
                renderer?.let { remoteVideo?.removeSink(it) }; frameProbe?.let { remoteVideo?.removeSink(it) }
                remoteVideo = track; renderer?.let { track.addSink(it) }
                frameProbe = VideoSink { submit { if (epoch == transportGeneration && !receivedFrame) { receivedFrame = true; status("first_frame") } } }
                track.addSink(frameProbe)
            }
            track?.setEnabled(true)
        } }
        override fun onConnectionChange(state: PeerConnection.PeerConnectionState) { submit {
            if (epoch != transportGeneration) return@submit
            if (state == PeerConnection.PeerConnectionState.CONNECTED || state == PeerConnection.PeerConnectionState.DISCONNECTED || state == PeerConnection.PeerConnectionState.FAILED) {
                signal("rtc.state", JSONObject().put("negotiationId", negotiation).put("state", state.name.lowercase()))
                if (state == PeerConnection.PeerConnectionState.CONNECTED) status("rtc_connected")
                else if (recoveryEnabled) status("rtc_reconnecting")
                else { status("RTC_CONNECTION_FAILED"); close() }
            }
        } }
        override fun onSignalingChange(state: PeerConnection.SignalingState) {}
        override fun onIceConnectionChange(state: PeerConnection.IceConnectionState) {}
        override fun onIceConnectionReceivingChange(receiving: Boolean) {}
        override fun onIceGatheringChange(state: PeerConnection.IceGatheringState) {}
        override fun onIceCandidatesRemoved(candidates: Array<IceCandidate>) {}
        override fun onAddStream(stream: MediaStream) {}
        override fun onRemoveStream(stream: MediaStream) {}
        override fun onDataChannel(channel: DataChannel) { channel.close(); channel.dispose() }
        override fun onRenegotiationNeeded() {}
    }
    private val stats = object : Runnable {
        override fun run() {
            if (closed.get()) return
            peer?.getStats { report ->
                val measured = JSONObject().put("timestampUs", report.timestampUs)
                report.statsMap.values.filter { it.type == "inbound-rtp" || it.type == "outbound-rtp" }.forEach {
                    val values = JSONObject()
                    for (key in listOf("bytesReceived", "bytesSent", "framesDecoded", "framesEncoded", "packetsLost", "jitter", "framesPerSecond")) it.members[key]?.let { value -> values.put(key, value) }
                    measured.put(it.id, values)
                }
                status("statistics:" + measured.toString())
            }
            worker.postDelayed(this, 1000)
        }
    }
    override fun close() {
        if (!closed.compareAndSet(false, true)) return
        playback?.stop()
        worker.removeCallbacksAndMessages(null)
        worker.post {
            runCatching { renderer?.let { remoteVideo?.removeSink(it) } }
            runCatching { frameProbe?.let { remoteVideo?.removeSink(it) } }
            runCatching { capture?.stopCapture(); capture?.dispose() }
            runCatching { peer?.close(); peer?.dispose() }
            runCatching { videoTrack?.dispose(); audioTrack?.dispose(); videoSource?.dispose(); audioSource?.dispose() }
            runCatching { texture?.dispose(); factory?.dispose(); audio?.release(); playback?.release() }
            // A receiver renderer belongs to this instance and is replaced before the next session.
            runCatching { renderer?.release(); egl.release() }
            thread.quitSafely()
        }
    }
}
