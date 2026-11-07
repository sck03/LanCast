import ScreenCaptureKit
import CoreMedia
import LanCastContracts

/// Capture policy stays independent of source selection, signaling and stream lifetime.
enum MacCaptureConfiguration {
    static func make(contentRect: CGRect, pointPixelScale: Float, audio: Bool) throws -> SCStreamConfiguration {
        let width = Double(contentRect.width) * Double(pointPixelScale)
        let height = Double(contentRect.height) * Double(pointPixelScale)
        guard pointPixelScale.isFinite, pointPixelScale > 0,
              width.isFinite, height.isFinite, width >= 1, height >= 1,
              width < Double(Int.max), height < Double(Int.max) else {
            throw CastFailure.invalid("采集来源尺寸无效，请重新选择屏幕或窗口")
        }
        let size = FrameSize.fit(width: Int(width), height: Int(height), maxWidth: 1280, maxHeight: 720)
        let config = SCStreamConfiguration()
        config.width = Int(size.width); config.height = Int(size.height)
        config.minimumFrameInterval = CMTime(value: 1, timescale: 30)
        config.queueDepth = 3
        config.pixelFormat = kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange
        config.colorSpaceName = CGColorSpace.itur_709
        config.captureDynamicRange = .SDR
        config.preservesAspectRatio = true; config.scalesToFit = true; config.showsCursor = true
        config.capturesAudio = audio; config.sampleRate = 48000; config.channelCount = 2
        config.excludesCurrentProcessAudio = true
        return config
    }
}
