import ScreenCaptureKit
import CoreMedia
import LanCastContracts

/// Capture owns no signaling, credentials or player. The user explicitly selects a source.
final class MacCapture: NSObject, SCStreamOutput, SCStreamDelegate {
    struct Source: Identifiable { let id: String; let name: String; let filter: SCContentFilter }
    private var stream: SCStream?
    private var generation = UUID()
    var video: (CMSampleBuffer) -> Void = { _ in }
    var audio: (CMSampleBuffer) -> Void = { _ in }
    var failed: (String) -> Void = { _ in }
    static func sources() async throws -> [Source] {
        let content = try await SCShareableContent.excludingDesktopWindows(true, onScreenWindowsOnly: true)
        return content.displays.map { Source(id: "display-\($0.displayID)", name: "显示器 \($0.displayID) · \($0.width)×\($0.height)", filter: SCContentFilter(display: $0, excludingWindows: [])) } +
            content.windows.filter { $0.owningApplication?.bundleIdentifier != Bundle.main.bundleIdentifier && $0.frame.width > 1 && $0.frame.height > 1 }.map {
                Source(id: "window-\($0.windowID)", name: "\($0.owningApplication?.applicationName ?? "窗口") · \($0.title ?? "")", filter: SCContentFilter(desktopIndependentWindow: $0))
            }
    }
    @MainActor func start(source: Source, audio: Bool) async throws {
        let current = UUID(); generation = current
        let previous = stream; stream = nil; try? await previous?.stopCapture()
        guard generation == current else { return }
        let config = SCStreamConfiguration()
        config.width = 1280; config.height = 720; config.minimumFrameInterval = CMTime(value: 1, timescale: 30)
        config.queueDepth = 3; config.pixelFormat = kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange
        config.colorSpaceName = CGColorSpace.itur_709; config.showsCursor = true
        config.scalesToFit = true
        config.capturesAudio = audio; config.sampleRate = 48000; config.channelCount = 2; config.excludesCurrentProcessAudio = true
        let stream = SCStream(filter: source.filter, configuration: config, delegate: self); self.stream = stream
        try stream.addStreamOutput(self, type: .screen, sampleHandlerQueue: .main)
        if audio { try stream.addStreamOutput(self, type: .audio, sampleHandlerQueue: .main) }
        do { try await stream.startCapture() }
        catch { if generation == current { self.stream = nil }; throw error }
        if generation != current { try? await stream.stopCapture() }
    }
    @MainActor func stop() async {
        generation = UUID(); let old = stream; stream = nil; try? await old?.stopCapture()
    }
    func stream(_ stream: SCStream, didOutputSampleBuffer sampleBuffer: CMSampleBuffer, of type: SCStreamOutputType) {
        guard stream === self.stream, CMSampleBufferIsValid(sampleBuffer) else { return }
        if type == .screen {
            guard let attachments = CMSampleBufferGetSampleAttachmentsArray(sampleBuffer, createIfNecessary: false) as? [[SCStreamFrameInfo: Any]],
                  let raw = attachments.first?[.status] as? Int, raw == SCFrameStatus.complete.rawValue else { return }
            video(sampleBuffer)
        } else if type == .audio { audio(sampleBuffer) }
    }
    func stream(_ stream: SCStream, didStopWithError error: Error) {
        DispatchQueue.main.async { [weak self] in guard let self, self.stream === stream else { return }; self.stream = nil; self.failed(error.localizedDescription) }
    }
}
