import XCTest
@testable import LanCastContracts

final class SessionContractsTests: XCTestCase {
    func testFramesFitWithoutCroppingPortraitOrSquareSources() {
        let portrait = FrameSize.fit(width: 1080, height: 1920, maxWidth: 1280, maxHeight: 720)
        XCTAssertEqual(portrait.width, 720); XCTAssertEqual(portrait.height, 1280)
        let square = FrameSize.fit(width: 2048, height: 2048, maxWidth: 1280, maxHeight: 720)
        XCTAssertEqual(square.width, 720); XCTAssertEqual(square.height, 720)
        let small = FrameSize.fit(width: 321, height: 181, maxWidth: 1280, maxHeight: 720)
        XCTAssertEqual(small.width, 320); XCTAssertEqual(small.height, 180)
    }
    func testTicketRejectsExpiredAndClockRollback() throws {
        let now = Date(timeIntervalSince1970: 1000)
        let ticket = try BroadcastTicket(address: "192.168.1.2:8787", fingerprint: String(repeating: "a", count: 64), invite: "12345678", audio: true, now: now)
        try ticket.validate(now: now.addingTimeInterval(119))
        XCTAssertThrowsError(try ticket.validate(now: now.addingTimeInterval(120)))
        XCTAssertThrowsError(try ticket.validate(now: now.addingTimeInterval(-1)))
        XCTAssertEqual(try JSONDecoder().decode(BroadcastTicket.self, from: JSONEncoder().encode(ticket)), ticket)
        XCTAssertEqual(ticket.version, 2)
        for code in ["one-use", "1234567a", "123456789", ""] {
            XCTAssertThrowsError(try BroadcastTicket(address: "192.168.1.2:8787", fingerprint: String(repeating: "a", count: 64), invite: code, audio: false))
        }
    }
    func testPartialPinAndInvalidEndpointRejected() {
        for address in ["0.0.0.0:9", "example.org:9", "192.168.1.2:0", "192.168.1.2:65536", "256.1.1.1:9"] {
            XCTAssertThrowsError(try BroadcastTicket(address: address, fingerprint: String(repeating: "a", count: 64), invite: "12345678", audio: false))
        }
        XCTAssertThrowsError(try BroadcastTicket(address: "192.168.1.2:9", fingerprint: "abcd", invite: "12345678", audio: false))
    }
    func testStopAndReplacementRejectOldReplies() {
        var gate = SessionGate(); gate.begin(request: "old"); let old = gate.generation
        gate.stop(); gate.begin(request: "new")
        XCTAssertFalse(gate.matches(old))
        XCTAssertFalse(gate.accept(reply: "old", session: UUID().uuidString))
        XCTAssertTrue(gate.accept(reply: "new", session: UUID().uuidString))
        XCTAssertFalse(gate.accept(reply: "new", session: UUID().uuidString))
        gate.stop(); XCTAssertNil(gate.session)
    }
}
