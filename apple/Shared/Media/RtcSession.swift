import Foundation
import CoreMedia
import LiveKitWebRTC
import LanCastContracts

/// All SDP/ICE and lifetime mutations run here. Capture queues only submit bounded samples.
final class RtcSession: NSObject, LKRTCPeerConnectionDelegate, LKRTCVideoRenderer {
    // Process lifetime, shared by sender/receiver factories. Never clean up while peers exist.
    private static let sslReady = LKRTCInitializeSSL()
    private let queue = DispatchQueue(label: "dev.lancast.rtc")
    private let videoSlots = DispatchSemaphore(value: 2)
    private var factory: LKRTCPeerConnectionFactory?
    private var peer: LKRTCPeerConnection?
    private var source: LKRTCVideoSource?
    private var capturer: LKRTCVideoCapturer?
    private var remoteVideo: LKRTCVideoTrack?
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
    private let signal: (String, JSONObject) -> Void
    private let status: (String) -> Void
    private let track: (LKRTCVideoTrack?) -> Void
    private let statistics: (JSONObject) -> Void
    private let sending: Bool
    private let withAudio: Bool

    init(sending: Bool, audio: Bool, signal: @escaping (String, JSONObject) -> Void,
         status: @escaping (String) -> Void, track: @escaping (LKRTCVideoTrack?) -> Void = { _ in },
         statistics: @escaping (JSONObject) -> Void = { _ in }) {
        self.sending = sending; withAudio = audio; self.signal = signal; self.status = status; self.track = track
        self.statistics = statistics
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
            guard device.setEngineAvailability(LKRTCAudioEngineAvailability(isInputAvailable: false, isOutputAvailable: withAudio)) == 0,
                  device.setPlatformVoiceProcessingAllowed(false) == 0 else {
                throw CastFailure.invalid("AUDIO_OUTPUT_INITIALIZE_FAILED")
            }
            factory = receiver
        }
        let config = LKRTCConfiguration(); config.sdpSemantics = .unifiedPlan; config.bundlePolicy = .maxBundle
        config.iceServers = [] // LAN only; no external STUN, TURN or service account.
        peer = factory?.peerConnection(with: config, constraints: constraints, delegate: self)
        guard peer != nil else { throw CastFailure.invalid("RTC_INITIALIZE_FAILED") }
        let timer = DispatchSource.makeTimerSource(queue: queue); statsTimer = timer
        timer.schedule(deadline: .now() + 1, repeating: 1)
        timer.setEventHandler { [weak self] in self?.measure() }; timer.resume()
        queue.asyncAfter(deadline: .now() + 20) { [weak self] in
            guard let self, !self.closed else { return }
            if self.peer?.connectionState != .connected { self.fail("RTC_CONNECT_TIMEOUT") }
            else if !self.sending && !self.receivedFrame { self.fail("RTC_FIRST_FRAME_TIMEOUT") }
        }
    }
    private var constraints: LKRTCMediaConstraints { LKRTCMediaConstraints(mandatoryConstraints: nil, optionalConstraints: nil) }
    func start(profile: JSONObject) { submit { [self] in
        try initialize()
        guard let factory, let peer else { return }
        let source = factory.videoSource(forScreenCast: true); self.source = source
        maxWidth = Int32(clamping: min(3840, max(2, profile["width"] as? Int ?? 1280)))
        maxHeight = Int32(clamping: min(2160, max(2, profile["height"] as? Int ?? 720)))
        maxFps = Int32(clamping: min(60, max(1, profile["fps"] as? Int ?? 30)))
        capturer = LKRTCVideoCapturer(delegate: source)
        let video = factory.videoTrack(with: source, trackId: "screen")
        let sendOnly = LKRTCRtpTransceiverInit(); sendOnly.direction = .sendOnly; sendOnly.streamIds = ["lancast"]
        guard let transceiver = peer.addTransceiver(with: video, init: sendOnly) else { throw CastFailure.invalid("VIDEO_TRACK_FAILED") }
        let codecs = factory.rtpSenderCapabilities(forKind: "video").codecs.filter { $0.name.lowercased() == "h264" }
        guard !codecs.isEmpty else { throw CastFailure.invalid("H264_UNAVAILABLE") }
        try transceiver.setCodecPreferences(codecs, error: ())
        let parameters = transceiver.sender.parameters
        for encoding in parameters.encodings {
            encoding.maxBitrateBps = NSNumber(value: profile["bitrate"] as? Int ?? 3_000_000)
            encoding.maxFramerate = NSNumber(value: profile["fps"] as? Int ?? 30)
        }
        transceiver.sender.parameters = parameters
        if withAudio {
            let audioConstraints = LKRTCMediaConstraints(mandatoryConstraints: ["googEchoCancellation": "false", "googAutoGainControl": "false", "googNoiseSuppression": "false"], optionalConstraints: nil)
            let audio = factory.audioTrack(with: factory.audioSource(with: audioConstraints), trackId: "system-audio")
            guard peer.addTransceiver(with: audio, init: sendOnly) != nil else { throw CastFailure.invalid("AUDIO_TRACK_FAILED") }
        }
        negotiation = UUID().uuidString
        let current = negotiation
        peer.offer(for: constraints) { [weak self] description, error in
            self?.submit { guard let self, self.negotiation == current else { return }; try self.publish(description, error: error, type: "rtc.offer") }
        }
    } }
    func offer(_ body: JSONObject) { submit { [self] in
        try initialize()
        guard UUID(uuidString: body.string("negotiationId")) != nil else { throw CastFailure.invalid("INVALID_NEGOTIATION") }
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
        peer?.add(candidate) { [weak self] error in if error != nil { self?.submit { self?.fail("ICE_REJECTED") } } }
    }
    private func flushIce() { pendingIce.forEach(add); pendingIce.removeAll() }
    private func publish(_ description: LKRTCSessionDescription?, error: Error?, type: String) throws {
        if let error { throw error }
        guard let description else { throw CastFailure.invalid("SDP_FAILED") }
        let current = negotiation
        peer?.setLocalDescription(description) { [weak self] error in
            self?.submit {
                guard let self, self.negotiation == current else { return }; if let error { throw error }
                self.signal(type, ["sdp": description.sdp, "negotiationId": current]); self.localSent = true
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
        remoteVideo?.remove(self); remoteVideo = nil; track(nil)
        peer?.delegate = nil; peer?.close(); peer = nil
        _ = audioInput?.terminateDevice()
        capturer = nil; source = nil; factory = nil; audioInput = nil
    }
    func setSize(_ size: CGSize) {}
    func renderFrame(_ frame: LKRTCVideoFrame?) {
        guard frame != nil else { return }
        submit { [self] in if !receivedFrame { receivedFrame = true; status("first_frame") } }
    }
    func peerConnection(_ peerConnection: LKRTCPeerConnection, didGenerate candidate: LKRTCIceCandidate) {
        submit { [self] in
            if localSent { sendIce(candidate) } else {
                guard localIce.count < 128 else { throw CastFailure.invalid("ICE_QUEUE_FULL") }; localIce.append(candidate)
            }
        }
    }
    func peerConnection(_ peerConnection: LKRTCPeerConnection, didAdd rtpReceiver: LKRTCRtpReceiver, streams: [LKRTCMediaStream]) {
        submit { [self] in
            if let video = rtpReceiver.track as? LKRTCVideoTrack { remoteVideo?.remove(self); remoteVideo = video; video.add(self); track(video) }
        }
    }
    func peerConnection(_ peerConnection: LKRTCPeerConnection, didChange newState: LKRTCPeerConnectionState) {
        submit { [self] in
            if newState == .connected { status("rtc_connected") }
            if newState == .failed || newState == .disconnected { fail("RTC_CONNECTION_LOST") }
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
