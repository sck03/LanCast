import XCTest
@testable import LanCastContracts

final class SessionContractsTests: XCTestCase {
    func testTicketRejectsExpiredAndClockRollback() throws {
        let now = Date(timeIntervalSince1970: 1000)
        let ticket = try BroadcastTicket(address: "192.168.1.2:8787", fingerprint: String(repeating: "a", count: 64), invite: "one-use", audio: true, now: now)
        try ticket.validate(now: now.addingTimeInterval(119))
        XCTAssertThrowsError(try ticket.validate(now: now.addingTimeInterval(120)))
        XCTAssertThrowsError(try ticket.validate(now: now.addingTimeInterval(-1)))
        XCTAssertEqual(try JSONDecoder().decode(BroadcastTicket.self, from: JSONEncoder().encode(ticket)), ticket)
    }
    func testPartialPinAndInvalidEndpointRejected() {
        for address in ["0.0.0.0:9", "example.org:9", "192.168.1.2:0", "192.168.1.2:65536", "256.1.1.1:9"] {
            XCTAssertThrowsError(try BroadcastTicket(address: address, fingerprint: String(repeating: "a", count: 64), invite: "token", audio: false))
        }
        XCTAssertThrowsError(try BroadcastTicket(address: "192.168.1.2:9", fingerprint: "abcd", invite: "token", audio: false))
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
