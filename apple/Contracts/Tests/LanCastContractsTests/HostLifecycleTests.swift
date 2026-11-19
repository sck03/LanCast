import XCTest
@testable import LanCastContracts

final class HostLifecycleTests: XCTestCase {
    func testRefreshPreservesWindowAcrossReorderingAndNeverChoosesDisplay() {
        XCTAssertEqual(SourceSelection.reconcile(selected: "window:9", available: ["display:1", "window:9"]), "window:9")
        XCTAssertEqual(SourceSelection.reconcile(selected: "window:9", available: ["display:1"]), "")
        XCTAssertEqual(SourceSelection.reconcile(selected: "", available: ["display:1"]), "")
    }
    func testPreparationExpiryAndExtensionDeathReleaseTheHost() throws {
        let now = Date(timeIntervalSince1970: 1000), id = UUID()
        let prepared = BroadcastStatus(id: id, phase: .prepared, now: now)
        XCTAssertTrue(prepared.isLive(now: now.addingTimeInterval(119)))
        XCTAssertFalse(prepared.isLive(now: now.addingTimeInterval(120)))
        for phase in [BroadcastStatus.Phase.starting, .broadcasting] {
            let state = BroadcastStatus(id: id, phase: phase, now: now)
            XCTAssertTrue(state.isLive(now: now.addingTimeInterval(14)))
            XCTAssertFalse(state.isLive(now: now.addingTimeInterval(15)))
            XCTAssertFalse(state.isLive(now: now.addingTimeInterval(-1)))
            XCTAssertEqual(try JSONDecoder().decode(BroadcastStatus.self, from: JSONEncoder().encode(state)), state)
        }
        for phase in [BroadcastStatus.Phase.finished, .failed] {
            XCTAssertFalse(BroadcastStatus(id: id, phase: phase, now: now).isLive(now: now))
        }
    }
}
