import ReplayKit
import LiveKitWebRTC
import LanCastContracts

/// ReplayKit owns the capture grant. The extension owns its control/media lifetime.
final class SampleHandler: RPBroadcastSampleHandler {
    private var sender: SenderSession?
    private var stopped = false
    private var ticketID: UUID?
    private var heartbeat: Timer?
    private var phase: BroadcastStatus.Phase = .starting
    private let sampleSlots = DispatchSemaphore(value: 3)
    override func broadcastStarted(withSetupInfo setupInfo: [String: NSObject]?) {
        DispatchQueue.main.async { [weak self] in
            guard let self, !self.stopped else { return }
            do {
                let ticket = try BroadcastStore.consume()
                self.ticketID = ticket.id
                self.heartbeat = Timer.scheduledTimer(withTimeInterval: 5, repeats: true) { [weak self] _ in self?.publishStatus() }
                let sender = SenderSession(); self.sender = sender
                sender.ended = { [weak self] in self?.finish("接收连接已结束") }
                sender.readyForCapture = { [weak self] in self?.phase = .broadcasting; self?.publishStatus() }
                try sender.connect(ticket, mirror: true)
            } catch { self.finish(error.localizedDescription) }
        }
    }
    override func broadcastPaused() {
        // Pausing ends the session; resume requires a new user broadcast grant and invitation.
        DispatchQueue.main.async { [weak self] in self?.finish("广播已暂停，请重新开始") }
    }
    override func broadcastFinished() {
        DispatchQueue.main.async { [weak self] in self?.end(.finished) }
    }
    override func processSampleBuffer(_ sampleBuffer: CMSampleBuffer, with sampleBufferType: RPSampleBufferType) {
        guard sampleBufferType != .audioMic, sampleSlots.wait(timeout: .now()) == .success else { return }
        DispatchQueue.main.async { [weak self, sampleSlots] in
            defer { sampleSlots.signal() }
            guard let self, !self.stopped else { return }
            if sampleBufferType == .video {
                let orientation = CMGetAttachment(sampleBuffer, key: RPVideoSampleOrientationKey as CFString, attachmentModeOut: nil) as? NSNumber
                let rotation: LKRTCVideoRotation
                switch orientation?.intValue { case 3: rotation = ._180; case 6: rotation = ._90; case 8: rotation = ._270; default: rotation = ._0 }
                self.sender?.pushVideo(sampleBuffer, rotation: rotation)
            } else if sampleBufferType == .audioApp { self.sender?.pushAudio(sampleBuffer) }
        }
    }
    private func publishStatus() {
        guard !stopped, let ticketID else { return }
        if BroadcastStore.cancelled(ticketID) { finish("广播已停止"); return }
        do { try BroadcastStore.publish(ticketID, phase: phase) }
        catch { finish("无法更新广播状态，请重新开始") }
    }
    private func end(_ result: BroadcastStatus.Phase) {
        guard !stopped else { return }
        stopped = true; heartbeat?.invalidate(); heartbeat = nil
        sender?.stop(); sender = nil
        if let ticketID { try? BroadcastStore.publish(ticketID, phase: result) }
    }
    private func finish(_ message: String) {
        guard !stopped else { return }; end(.failed)
        finishBroadcastWithError(NSError(domain: "LanCast", code: 1, userInfo: [NSLocalizedDescriptionKey: message]))
    }
}
