import XCTest
import ScreenCaptureKit
@testable import LanCastMac

final class MacCaptureConfigurationTests: XCTestCase {
    func testNativePixelsPreservePortraitAndSmallWindowDimensions() throws {
        for (width, height, scale, expectedWidth, expectedHeight) in [
            (1920, 1080, 2, 1280, 720), (1080, 1920, 2, 720, 1280),
            (320, 240, 1, 320, 240), (320, 240, 2, 640, 480), (1000, 1000, 1, 720, 720)
        ] {
            let config = try MacCaptureConfiguration.make(
                contentRect: CGRect(x: 0, y: 0, width: CGFloat(width), height: CGFloat(height)),
                pointPixelScale: Float(scale), audio: false)
            XCTAssertEqual(config.width, expectedWidth); XCTAssertEqual(config.height, expectedHeight)
            XCTAssertTrue(config.preservesAspectRatio)
            XCTAssertEqual(config.captureDynamicRange, .SDR)
            XCTAssertFalse(config.capturesAudio)
        }
    }

    func testInvalidSourceCannotBecomeAnOversizedAllocation() {
        let scales: [Float] = [0, -1, .infinity, .nan]
        for scale in scales {
            XCTAssertThrowsError(try MacCaptureConfiguration.make(
                contentRect: CGRect(x: 0, y: 0, width: 1280, height: 720), pointPixelScale: scale, audio: true))
        }
        XCTAssertThrowsError(try MacCaptureConfiguration.make(contentRect: .zero, pointPixelScale: 1, audio: true))
    }
}
