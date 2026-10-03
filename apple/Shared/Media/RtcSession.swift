import Foundation
import CoreMedia
import LiveKitWebRTC
import LanCastContracts

private final class RtcFrameProbe: NSObject, LKRTCVideoRenderer {
    let frame: () -> Void
    init(frame: @escaping () -> Void) { self.frame = frame }
    func setSize(_ size: CGSize) {}
    func renderFrame(_ value: LKRTCVideoFrame?) { if value != nil { frame() } }
}

/// All SDP/ICE and lifetime mutations run here. Capture queues only submit bounded samples.
final class RtcSession: NSObject, LKRTCPeerConnectionDelegate {
    // Process lifetime, shared by sender/receiver factories. Never clean up while peers exist.
    private static let sslReady = LKRTCInitializeSSL()
    private let queue = DispatchQueue(label: "dev.lancast.rtc")
    private let videoSlots = DispatchSemaphore(value: 2)
    private var factory: LKRTCPeerConnectionFactory?
    private var peer: LKRTCPeerConnection?
    private var source: LKRTCVideoSource?
    private var capturer: LKRTCVideoCapturer?
    private var remoteVideo: LKRTCVideoTrack?
    private var frameProbe: RtcFrameProbe?
    private var audioInput: LCSystemAudioDevice?
    private var negotiation = ""
    private var remoteSet = false
    private var localSent = false
    private var pendingIce: [LKRTCIceCandidate] = []
    private var localIce: [LKRTCIceCandidate] = []
    private var closed = false
    private var receivedFrame = false
    private var statsTimer: DispatchSourceTimer?
    private var frameSize: FrameSize?
    private var maxWidth: Int32 = 1280
    private var maxHeight: Int32 = 720
    private var maxFps: Int32 = 30
    private var videoTrack: LKRTCVideoTrack?
    private var audioTrack: LKRTCAudioTrack?
    private var senderProfile: JSONObject = [:]
    private var transportGeneration = UUID()
    private let signal: (String, JSONObject) -> Void
    private let status: (String) -> Void
    private let track: (LKRTCVideoTrack?) -> Void
    private let statistics: (JSONObject) -> Void
    private let sending: Bool
    private let withAudio: Bool
    private let recoveryEnabled: Bool

    init(sending: Bool, audio: Bool, signal: @escaping (String, JSONObject) -> Void,
         status: @escaping (String) -> Void, track: @escaping (LKRTCVideoTrack?) -> Void = { _ in },
         statistics: @escaping (JSONObject) -> Void = { _ in }, recoveryEnabled: Bool = false) {
        self.sending = sending; withAudio = audio; self.signal = signal; self.status = status; self.track = track
        self.statistics = statistics
        self.recoveryEnabled = recoveryEnabled
        super.init()
    }
    private func submit(_ action: @escaping () throws -> Void) {
        queue.async { [weak self] in
            guard let self, !self.closed else { return }
            do { try action() } catch { self.fail(error.localizedDescription) }
        }
    }
    private func initialize() throws {
        guard peer == nil else { return }
        guard Self.sslReady else { throw CastFailure.invalid("RTC_SSL_INITIALIZE_FAILED") }
        let encoder = LKRTCVideoEncoderFactoryH264()
        let decoder = LKRTCVideoDecoderFactoryH264()
        if sending {
            let input = LCSystemAudioDevice(); audioInput = input
            factory = LKRTCPeerConnectionFactory(encoderFactory: encoder, decoderFactory: decoder, audioDevice: input)
        } else {
            let receiver = LKRTCPeerConnectionFactory(audioDeviceModuleType: .audioEngine, bypassVoiceProcessing: true,
                encoderFactory: encoder, decoderFactory: decoder, audioProcessingModule: nil)
            let device = receiver.audioDeviceModule
            guard device.setEngineAvailability(LKRTCAudioEngineAvailability(isInputAvailable: false, isOutputAvailable: ObjCBool(withAudio))) == 0,
                  device.setPlatformVoiceProcessingAllowed(false) == 0 else {
                throw CastFailure.invalid("AUDIO_OUTPUT_INITIALIZE_FAILED")
            }
            factory = receiver
        }
        try createPeer()
        let timer = DispatchSource.makeTimerSource(queue: queue); statsTimer = timer
        timer.schedule(deadline: .now() + 1, repeating: 1)
        timer.setEventHandler { [weak self] in self?.measure() }; timer.resume()
    }
    private func createPeer() throws {
        transportGeneration = UUID(); let current = transportGeneration
        remoteSet = false; localSent = false; receivedFrame = false
        pendingIce.removeAll(); localIce.removeAll()
        let config = LKRTCConfiguration(); config.sdpSemantics = .unifiedPlan; config.bundlePolicy = .maxBundle
        config.iceServers = [] // LAN only; no external STUN, TURN or service account.
        peer = factory?.peerConnection(with: config, constraints: constraints, delegate: self)
        guard peer != nil else { throw CastFailure.invalid("RTC_INITIALIZE_FAILED") }
        queue.asyncAfter(deadline: .now() + 20) { [weak self] in
            guard let self, !self.closed, self.transportGeneration == current else { return }
            if self.peer?.connectionState != .connected { self.fail("RTC_CONNECT_TIMEOUT") }
            else if !self.sending && !self.receivedFrame { self.fail("RTC_FIRST_FRAME_TIMEOUT") }
        }
    }
    private func clearPeer() {
        transportGeneration = UUID()
        if let frameProbe { remoteVideo?.remove(frameProbe) }; frameProbe = nil
        remoteVideo = nil; track(nil)
        peer?.delegate = nil; peer?.close(); peer = nil
    }
    private var constraints: LKRTCMediaConstraints { LKRTCMediaConstraints(mandatoryConstraints: nil, optionalConstraints: nil) }
    func start(profile: JSONObject) { submit { [self] in
        try initialize()
        guard let factory, let peer else { return }
        senderProfile = profile
        let source = factory.videoSource(forScreenCast: true); self.source = source
        maxWidth = Int32(clamping: min(3840, max(2, profile["width"] as? Int ?? 1280)))
        maxHeight = Int32(clamping: min(2160, max(2, profile["height"] as? Int ?? 720)))
        maxFps = Int32(clamping: min(60, max(1, profile["fps"] as? Int ?? 30)))
        capturer = LKRTCVideoCapturer(delegate: source)
        let video = factory.videoTrack(with: source, trackId: "screen"); videoTrack = video
        if withAudio {
            let audioConstraints = LKRTCMediaConstraints(mandatoryConstraints: ["googEchoCancellation": "false", "googAutoGainControl": "false", "googNoiseSuppression": "false"], optionalConstraints: nil)
            audioTrack = factory.audioTrack(with: factory.audioSource(with: audioConstraints), trackId: "system-audio")
        }
        try attachTracks()
        negotiation = UUID().uuidString
        let current = negotiation
        peer.offer(for: constraints) { [weak self] description, error in
            self?.submit { guard let self, self.negotiation == current else { return }; try self.publish(description, error: error, type: "rtc.offer") }
        }
    } }
    private func attachTracks() throws {
        guard let factory, let peer, let video = videoTrack else { throw CastFailure.invalid("VIDEO_TRACK_FAILED") }
        let sendOnly = LKRTCRtpTransceiverInit(); sendOnly.direction = .sendOnly; sendOnly.streamIds = ["lancast"]
        guard let transceiver = peer.addTransceiver(with: video, init: sendOnly) else { throw CastFailure.invalid("VIDEO_TRACK_FAILED") }
        let codecs = factory.rtpSenderCapabilities(forKind: "video").codecs.filter { $0.name.lowercased() == "h264" }
        guard !codecs.isEmpty else { throw CastFailure.invalid("H264_UNAVAILABLE") }
        try transceiver.setCodecPreferences(codecs, error: ())
        let parameters = transceiver.sender.parameters
        for encoding in parameters.encodings {
            encoding.maxBitrateBps = NSNumber(value: senderProfile["bitrate"] as? Int ?? 3_000_000)
            encoding.maxFramerate = NSNumber(value: senderProfile["fps"] as? Int ?? 30)
        }
        transceiver.sender.parameters = parameters
        if let audio = audioTrack {
            guard peer.addTransceiver(with: audio, init: sendOnly) != nil else { throw CastFailure.invalid("AUDIO_TRACK_FAILED") }
        }
    }
    func restart(previous: String) { submit { [self] in
        guard sending, recoveryEnabled, previous == negotiation else { return }
        // Capture and the system audio device survive transport replacement.
        clearPeer(); try createPeer(); try attachTracks()
        negotiation = UUID().uuidString
        let current = negotiation
        peer?.offer(for: constraints) { [weak self] description, error in
            self?.submit { guard let self, self.negotiation == current else { return }; try self.publish(description, error: error, type: "rtc.offer", previous: previous) }
        }
        status("rtc_reconnecting")
    } }
    func offer(_ body: JSONObject) { submit { [self] in
        try initialize()
        guard UUID(uuidString: body.string("negotiationId")) != nil else { throw CastFailure.invalid("INVALID_NEGOTIATION") }
        if !negotiation.isEmpty {
            guard negotiation != body.string("negotiationId") else { return }
            guard recoveryEnabled else { throw CastFailure.invalid("RECOVERY_NOT_NEGOTIATED") }
            clearPeer(); try createPeer()
        }
        negotiation = body.string("negotiationId"); remoteSet = false; localSent = false
        pendingIce.removeAll(); localIce.removeAll()
        let current = negotiation
        peer?.setRemoteDescription(LKRTCSessionDescription(type: .offer, sdp: body.string("sdp"))) { [weak self] error in
            self?.submit {
                guard let self, self.negotiation == current else { return }
                if let error { throw error }; self.remoteSet = true; self.flushIce()
                self.peer?.answer(for: self.constraints) { [weak self] description, error in
                    self?.submit { guard let self, self.negotiation == current else { return }; try self.publish(description, error: error, type: "rtc.answer") }
                }
            }
        }
    } }
    func answer(_ body: JSONObject) { submit { [self] in
        guard body.string("negotiationId") == negotiation else { return }
        let current = negotiation
        peer?.setRemoteDescription(LKRTCSessionDescription(type: .answer, sdp: body.string("sdp"))) { [weak self] error in
            self?.submit { guard let self, self.negotiation == current else { return }; if let error { throw error }; self.remoteSet = true; self.flushIce() }
        }
    } }
    func ice(_ body: JSONObject) { submit { [self] in
        guard body.string("negotiationId") == negotiation else { return }
        let candidate = LKRTCIceCandidate(sdp: body.string("candidate"), sdpMLineIndex: Int32(body["sdpMLineIndex"] as? Int ?? 0), sdpMid: body["sdpMid"] as? String)
        if remoteSet { add(candidate) } else {
            guard pendingIce.count < 128 else { throw CastFailure.invalid("ICE_QUEUE_FULL") }
            pendingIce.append(candidate)
        }
    } }
    private func add(_ candidate: LKRTCIceCandidate) {
        let current = negotiation
        peer?.add(candidate) { [weak self] error in if error != nil { self?.submit { guard let self, self.negotiation == current else { return }; self.fail("ICE_REJECTED") } } }
    }
    private func flushIce() { pendingIce.forEach(add); pendingIce.removeAll() }
    private func publish(_ description: LKRTCSessionDescription?, error: Error?, type: String, previous: String? = nil) throws {
        if let error { throw error }
        guard let description else { throw CastFailure.invalid("SDP_FAILED") }
        let current = negotiation
        peer?.setLocalDescription(description) { [weak self] error in
            self?.submit {
                guard let self, self.negotiation == current else { return }; if let error { throw error }
                var body: JSONObject = ["sdp": description.sdp, "negotiationId": current]
                if let previous { body["previousNegotiationId"] = previous }
                self.signal(type, body); self.localSent = true
                self.localIce.forEach(self.sendIce); self.localIce.removeAll()
            }
        }
    }
    private func sendIce(_ ice: LKRTCIceCandidate) {
        signal("rtc.ice", ["candidate": ice.sdp, "sdpMid": ice.sdpMid ?? "0", "sdpMLineIndex": ice.sdpMLineIndex, "negotiationId": negotiation])
    }
    func pushVideo(_ sample: CMSampleBuffer, rotation: LKRTCVideoRotation = ._0) {
        guard CMSampleBufferIsValid(sample), videoSlots.wait(timeout: .now()) == .success else { return }
        queue.async { [weak self, videoSlots] in
            defer { videoSlots.signal() }
            guard let self, !self.closed, let source = self.source, let capturer = self.capturer,
                  let pixel = CMSampleBufferGetImageBuffer(sample) else { return }
            let seconds = CMTimeGetSeconds(CMSampleBufferGetPresentationTimeStamp(sample))
            guard seconds.isFinite, seconds >= 0, seconds < Double(Int64.max) / 1e9 else { return }
            let size = FrameSize.fit(width: CVPixelBufferGetWidth(pixel), height: CVPixelBufferGetHeight(pixel), maxWidth: self.maxWidth, maxHeight: self.maxHeight)
            if self.frameSize != size {
                self.frameSize = size
                source.adaptOutputFormat(toWidth: size.width, height: size.height, fps: self.maxFps)
            }
            let frame = LKRTCVideoFrame(buffer: LKRTCCVPixelBuffer(pixelBuffer: pixel), rotation: rotation, timeStampNs: Int64(seconds * 1e9))
            source.capturer(capturer, didCapture: frame)
        }
    }
    func pushAudio(_ sample: CMSampleBuffer) {
        // Access is serialized with creation and stop; audio device has its own bounded queue.
        guard withAudio, videoSlots.wait(timeout: .now()) == .success else { return }
        queue.async { [weak self, videoSlots] in
            defer { videoSlots.signal() }; guard let self, !self.closed else { return }; self.audioInput?.pushSample(sample)
        }
    }
    private func measure() {
        guard !closed else { return }
        peer?.statistics { [weak self] report in
            self?.submit {
                guard let self else { return }
                var values: JSONObject = [:]
                for (id, stat) in report.statistics where stat.type == "inbound-rtp" || stat.type == "outbound-rtp" {
                    values[id] = stat.values.filter { ["bytesReceived", "bytesSent", "framesDecoded", "framesEncoded", "packetsLost", "jitter"].contains($0.key) }
                }
                self.statistics(values)
            }
        }
    }
    private func fail(_ message: String) { status(message); dispose() }
    func close() { queue.async { [self] in dispose() } }
    private func dispose() {
        guard !closed else { return }; closed = true
        statsTimer?.cancel(); statsTimer = nil
        clearPeer()
        _ = audioInput?.terminateDevice()
        capturer = nil; source = nil; videoTrack = nil; audioTrack = nil; factory = nil; audioInput = nil
    }
    func peerConnection(_ peerConnection: LKRTCPeerConnection, didGenerate candidate: LKRTCIceCandidate) {
        submit { [self] in
            guard peerConnection === peer else { return }
            if localSent { sendIce(candidate) } else {
                guard localIce.count < 128 else { throw CastFailure.invalid("ICE_QUEUE_FULL") }; localIce.append(candidate)
            }
        }
    }
    func peerConnection(_ peerConnection: LKRTCPeerConnection, didAdd rtpReceiver: LKRTCRtpReceiver, streams: [LKRTCMediaStream]) {
        submit { [self] in
            guard peerConnection === peer else { return }
            if let video = rtpReceiver.track as? LKRTCVideoTrack {
                if let frameProbe { remoteVideo?.remove(frameProbe) }
                let current = transportGeneration
                let probe = RtcFrameProbe { [weak self] in
                    self?.submit {
                        guard let self, self.transportGeneration == current, !self.receivedFrame else { return }
                        self.receivedFrame = true; self.status("first_frame")
                    }
                }
                frameProbe = probe; remoteVideo = video; video.add(probe); track(video)
            }
        }
    }
    func peerConnection(_ peerConnection: LKRTCPeerConnection, didChange newState: LKRTCPeerConnectionState) {
        submit { [self] in
            guard peerConnection === peer else { return }
            if newState == .connected || newState == .failed || newState == .disconnected {
                signal("rtc.state", ["negotiationId": negotiation, "state": newState == .connected ? "connected" : (newState == .failed ? "failed" : "disconnected")])
                if newState == .connected { status("rtc_connected") }
                else if recoveryEnabled { status("rtc_reconnecting") }
                else { fail("RTC_CONNECTION_LOST") }
            }
        }
    }
    func peerConnection(_ peerConnection: LKRTCPeerConnection, didChange stateChanged: LKRTCSignalingState) {}
    func peerConnection(_ peerConnection: LKRTCPeerConnection, didAdd stream: LKRTCMediaStream) {}
    func peerConnection(_ peerConnection: LKRTCPeerConnection, didRemove stream: LKRTCMediaStream) {}
    func peerConnectionShouldNegotiate(_ peerConnection: LKRTCPeerConnection) {}
    func peerConnection(_ peerConnection: LKRTCPeerConnection, didChange newState: LKRTCIceConnectionState) {}
    func peerConnection(_ peerConnection: LKRTCPeerConnection, didChange newState: LKRTCIceGatheringState) {}
    func peerConnection(_ peerConnection: LKRTCPeerConnection, didRemove candidates: [LKRTCIceCandidate]) {}
    func peerConnection(_ peerConnection: LKRTCPeerConnection, didOpen dataChannel: LKRTCDataChannel) { dataChannel.close() }
}
