import XCTest
import CoreMedia
import CoreVideo
import LiveKitWebRTC
@testable import LanCastMac

final class FrameProbe: NSObject, LKRTCVideoRenderer {
    let received: XCTestExpectation
    private let lock = NSLock()
    private var done = false
    init(_ received: XCTestExpectation) { self.received = received }
    func setSize(_ size: CGSize) {}
    func renderFrame(_ frame: LKRTCVideoFrame?) {
        guard let frame else { return }
        lock.lock(); defer { lock.unlock() }; guard !done else { return }; done = true
        XCTAssertEqual(frame.width, 320); XCTAssertEqual(frame.height, 180)
        received.fulfill()
    }
}

final class RtcIntegrationTests: XCTestCase {
    func testH264SyntheticFramesCrossIceDtlsSrtpAndDecode() throws {
        let decoded = expectation(description: "received H264 video frame")
        let recovered = expectation(description: "decoded H264 after replacing both transports")
        let probe = FrameProbe(decoded)
        let recoveryProbe = FrameProbe(recovered)
        let signalLock = NSLock()
        var latestOffer: JSONObject = [:]
        var firstAnswer: JSONObject = [:]
        var trackCount = 0
        var tx: RtcSession!; var rx: RtcSession!
        rx = RtcSession(sending: false, audio: false, signal: { type, body in
            if type == "rtc.answer" {
                signalLock.lock(); if firstAnswer.isEmpty { firstAnswer = body }; signalLock.unlock()
                tx.answer(body)
            }; if type == "rtc.ice" { tx.ice(body) }
        }, status: { status in
            if status != "first_frame" && status != "rtc_connected" && status != "rtc_reconnecting" { XCTFail(status) }
        }, track: { track in
            guard let track else { return }; trackCount += 1
            track.add(trackCount == 1 ? probe : recoveryProbe)
        }, recoveryEnabled: true)
        tx = RtcSession(sending: true, audio: false, signal: { type, body in
            if type == "rtc.offer" {
                signalLock.lock(); latestOffer = body; signalLock.unlock()
                rx.offer(body)
            }; if type == "rtc.ice" { rx.ice(body) }
        }, status: { if $0 != "rtc_connected" && $0 != "rtc_reconnecting" { XCTFail($0) } }, recoveryEnabled: true)
        let timer = DispatchSource.makeTimerSource(queue: DispatchQueue(label: "test.frames"))
        var frameNumber: Int64 = 0
        timer.schedule(deadline: .now() + .milliseconds(100), repeating: .milliseconds(33))
        timer.setEventHandler {
            var pixel: CVPixelBuffer?
            let attributes: [CFString: Any] = [kCVPixelBufferIOSurfacePropertiesKey: [:]]
            guard CVPixelBufferCreate(nil, 320, 180, kCVPixelFormatType_32BGRA, attributes as CFDictionary, &pixel) == kCVReturnSuccess, let pixel else { return }
            CVPixelBufferLockBaseAddress(pixel, [])
            if let base = CVPixelBufferGetBaseAddress(pixel) {
                memset(base, 0x80, CVPixelBufferGetBytesPerRow(pixel) * 180)
            }
            CVPixelBufferUnlockBaseAddress(pixel, [])
            var format: CMVideoFormatDescription?
            CMVideoFormatDescriptionCreateForImageBuffer(allocator: nil, imageBuffer: pixel, formatDescriptionOut: &format)
            guard let format else { return }
            var timing = CMSampleTimingInfo(duration: CMTime(value: 1, timescale: 30), presentationTimeStamp: CMTime(value: frameNumber, timescale: 30), decodeTimeStamp: .invalid)
            frameNumber += 1
            var sample: CMSampleBuffer?
            CMSampleBufferCreateReadyWithImageBuffer(allocator: nil, imageBuffer: pixel, formatDescription: format, sampleTiming: &timing, sampleBufferOut: &sample)
            if let sample { tx.pushVideo(sample) }
        }
        defer { timer.cancel(); tx.close(); rx.close() }
        tx.start(profile: ["width": 320, "height": 180, "fps": 30, "bitrate": 500_000]); timer.resume()
        wait(for: [decoded], timeout: 25)
        signalLock.lock(); let previous = latestOffer.string("negotiationId"); let stale = firstAnswer; signalLock.unlock()
        XCTAssertFalse(previous.isEmpty)
        tx.restart(previous: previous)
        // Delivered after the restart request; it must never mutate the new peer.
        tx.answer(stale)
        wait(for: [recovered], timeout: 25)
        signalLock.lock(); let next = latestOffer; signalLock.unlock()
        XCTAssertNotEqual(next.string("negotiationId"), previous)
        XCTAssertEqual(next.string("previousNegotiationId"), previous)
    }
}
