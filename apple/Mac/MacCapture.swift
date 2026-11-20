import ScreenCaptureKit
import CoreMedia

/// Capture owns no signaling, credentials or player. The user explicitly selects a source.
final class MacCapture: NSObject, SCStreamOutput, SCStreamDelegate {
    struct Source: Identifiable { let id: String; let name: String; let filter: SCContentFilter }
    private var stream: SCStream?
    private var generation = UUID()
    private let outputQueue = DispatchQueue(label: "dev.lancast.capture", qos: .userInitiated)
    // These fields are accessed exclusively on outputQueue.
    private var outputStream: SCStream?
    private var videoSink: (CMSampleBuffer) -> Void = { _ in }
    private var audioSink: (CMSampleBuffer) -> Void = { _ in }
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
        let previous = stream; stream = nil
        outputQueue.sync { outputStream = nil }
        try? await previous?.stopCapture()
        guard generation == current else { return }
        let config = try MacCaptureConfiguration.make(contentRect: source.filter.contentRect,
            pointPixelScale: source.filter.pointPixelScale, audio: audio)
        let stream = SCStream(filter: source.filter, configuration: config, delegate: self); self.stream = stream
        let video = self.video, audioSink = self.audio
        outputQueue.sync { self.outputStream = stream; self.videoSink = video; self.audioSink = audioSink }
        do {
            try stream.addStreamOutput(self, type: .screen, sampleHandlerQueue: outputQueue)
            if audio { try stream.addStreamOutput(self, type: .audio, sampleHandlerQueue: outputQueue) }
            try await stream.startCapture()
        }
        catch { if generation == current { self.stream = nil; outputQueue.sync { outputStream = nil } }; throw error }
        if generation != current { try? await stream.stopCapture() }
    }
    @MainActor func stop() async {
        generation = UUID(); let old = stream; stream = nil
        outputQueue.sync { outputStream = nil; videoSink = { _ in }; audioSink = { _ in } }
        try? await old?.stopCapture()
    }
    func stream(_ stream: SCStream, didOutputSampleBuffer sampleBuffer: CMSampleBuffer, of type: SCStreamOutputType) {
        guard stream === outputStream, CMSampleBufferIsValid(sampleBuffer) else { return }
        if type == .screen {
            guard let attachments = CMSampleBufferGetSampleAttachmentsArray(sampleBuffer, createIfNecessary: false) as? [[SCStreamFrameInfo: Any]],
                  let raw = attachments.first?[.status] as? Int, raw == SCFrameStatus.complete.rawValue else { return }
            videoSink(sampleBuffer)
        } else if type == .audio { audioSink(sampleBuffer) }
    }
    func stream(_ stream: SCStream, didStopWithError error: Error) {
        DispatchQueue.main.async { [weak self] in guard let self, self.stream === stream else { return }; self.stream = nil; self.outputQueue.sync { self.outputStream = nil }; self.failed(error.localizedDescription) }
    }
}
