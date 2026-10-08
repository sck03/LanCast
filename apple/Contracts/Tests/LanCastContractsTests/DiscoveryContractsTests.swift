import XCTest
@testable import LanCastContracts

final class DiscoveryContractsTests: XCTestCase {
    func testHintsRejectUnsafeEndpointsAndInvalidPins() {
        for ip in ["192.168.1.3", "172.16.0.3", "10.0.0.5"] { XCTAssertTrue(ConnectionHints.isLanIPv4(ip)) }
        for ip in ["0.0.0.0", "127.0.0.1", "8.8.8.8", "host.local", "192.168.001.3", "999.1.2.3"] { XCTAssertFalse(ConnectionHints.isLanIPv4(ip)) }
        XCTAssertEqual(ConnectionHints.fingerprint(String(repeating: "AA:", count: 31) + "AA"), String(repeating: "a", count: 64))
        XCTAssertNil(ConnectionHints.fingerprint(String(repeating: "g", count: 64)))
        XCTAssertNil(DiscoveredReceiver(record: ["id": "bad", "port": "oops", "addresses": ["192.168.1.1"]]))
    }
    func testDiscoverySelectsIPv4AndOldReceiverClearsPin() {
        var record: [String: Any] = ["id": "tv", "name": "客厅._lancast._tcp.local.", "port": 8787, "addresses": ["::1", "192.168.1.9"], "fingerprint": String(repeating: "b", count: 64)]
        let found = DiscoveredReceiver.parse([record, record])
        XCTAssertEqual(found.count, 1); XCTAssertEqual(found[0].name, "客厅")
        XCTAssertEqual(found[0].address, "192.168.1.9:8787")
        record.removeValue(forKey: "fingerprint")
        XCTAssertEqual(DiscoveredReceiver(record: record)?.fingerprint, "")
        record["addresses"] = ["::1"]
        XCTAssertNil(DiscoveredReceiver(record: record))
    }
}
