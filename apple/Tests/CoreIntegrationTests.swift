import XCTest
@testable import LanCastMac

final class CoreIntegrationTests: XCTestCase {
    func testPinnedPairingRequiresApprovalAndStopsSession() throws {
        try verifyStop(closeConnection: false)
    }
    func testClosingAuthenticatedSenderRevokesReceiverSession() throws {
        try verifyStop(closeConnection: true)
    }
    private func verifyStop(closeConnection: Bool) throws {
        let active = expectation(description: "authenticated session reached receiver")
        let ended = expectation(description: "stop reached receiver")
        var receiver: CoreSession!
        var sender: CoreSession!
        var approvalSeen = false
        receiver = try CoreSession { event in
            let body = event.object("body")
            switch event.string("type") {
            case "receiver.ready": sender.command("connect", ["address": body.string("address"), "fingerprint": body.string("fingerprint"), "invite": body.string("invite"), "name": "Apple integration"])
            case "pair.request": approvalSeen = true; receiver.command("approve", ["connectionId": body.string("connectionId"), "accept": true])
            case "session.started": XCTAssertTrue(approvalSeen); active.fulfill()
            case "message": if body.string("type") == "session.stop" { ended.fulfill() }
            case "session.closed": if closeConnection { ended.fulfill() }
            case "error": XCTFail(body.string("code"))
            default: break
            }
        }
        sender = try CoreSession { event in
            let body = event.object("body")
            if event.string("type") == "connected" { sender.send("session.start", session: nil, body: ["mode": "mirror", "audioRequested": false]) }
            if event.string("type") == "message", body.string("type") == "session.accepted" {
                if closeConnection { sender.close() }
                else { sender.send("session.stop", session: body.string("sessionId"), body: ["reason": "test_complete"]) }
            }
            if event.string("type") == "error" { XCTFail(body.string("code")) }
        }
        defer { sender.close(); receiver.close(); sender = nil; receiver = nil }
        receiver.command("listen", ["address": "127.0.0.1:0", "name": "LanCast XCTest", "variant": "apple"])
        wait(for: [active, ended], timeout: 15)
    }
    func testWrongPinNeverReachesApproval() throws {
        let rejected = expectation(description: "wrong TLS pin rejected")
        var receiver: CoreSession!
        var sender: CoreSession!
        receiver = try CoreSession { event in
            let body = event.object("body")
            if event.string("type") == "receiver.ready" { sender.command("connect", ["address": body.string("address"), "fingerprint": String(repeating: "0", count: 64), "invite": body.string("invite"), "name": "Wrong pin"]) }
            if event.string("type") == "pair.request" { XCTFail("Unverified TLS client reached approval") }
        }
        sender = try CoreSession { event in if event.string("type") == "error" { rejected.fulfill() } }
        defer { sender.close(); receiver.close(); sender = nil; receiver = nil }
        receiver.command("listen", ["address": "127.0.0.1:0", "name": "LanCast pin test"])
        wait(for: [rejected], timeout: 15)
    }
}
