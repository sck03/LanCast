import SwiftUI
import LiveKitWebRTC

final class VideoBinding {
    var track: LKRTCVideoTrack?
    func update(_ new: LKRTCVideoTrack?, view: LKRTCMTLVideoView) {
        if track !== new { track?.remove(view); track = new; new?.add(view) }
    }
}
#if os(macOS)
struct VideoSurface: NSViewRepresentable {
    var track: LKRTCVideoTrack?
    func makeCoordinator() -> VideoBinding { VideoBinding() }
    func makeNSView(context: Context) -> LKRTCMTLVideoView { LKRTCMTLVideoView(frame: .zero) }
    func updateNSView(_ view: LKRTCMTLVideoView, context: Context) { context.coordinator.update(track, view: view) }
    static func dismantleNSView(_ view: LKRTCMTLVideoView, coordinator: VideoBinding) { coordinator.update(nil, view: view) }
}
#else
struct VideoSurface: UIViewRepresentable {
    var track: LKRTCVideoTrack?
    func makeCoordinator() -> VideoBinding { VideoBinding() }
    func makeUIView(context: Context) -> LKRTCMTLVideoView { let view = LKRTCMTLVideoView(frame: .zero); view.videoContentMode = .scaleAspectFit; return view }
    func updateUIView(_ view: LKRTCMTLVideoView, context: Context) { context.coordinator.update(track, view: view) }
    static func dismantleUIView(_ view: LKRTCMTLVideoView, coordinator: VideoBinding) { coordinator.update(nil, view: view) }
}
#endif
