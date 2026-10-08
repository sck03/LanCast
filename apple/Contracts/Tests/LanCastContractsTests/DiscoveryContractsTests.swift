import XCTest
@testable import LanCastContracts

final class DiscoveryContractsTests: XCTestCase {
    func testBonjourUsesTheAdvertisedBoundAddressAndCurrentVersion() {
        var txt = ["deviceId": "tv", "pairingVersion": "2", "address": "10.0.0.9", "fingerprint": String(repeating: "a", count: 64)].mapValues { Data($0.utf8) }
        let receiver = DiscoveredReceiver.bonjour(name: "TV", port: 8787, resolved: ["192.168.1.9", "10.0.0.9"], txt: txt)
        XCTAssertEqual(receiver?.address, "10.0.0.9:8787")
        XCTAssertNil(DiscoveredReceiver.bonjour(name: "TV", port: 8787, resolved: ["192.168.1.9"], txt: txt))
        txt["pairingVersion"] = Data("1".utf8)
        XCTAssertNil(DiscoveredReceiver.bonjour(name: "TV", port: 8787, resolved: ["10.0.0.9"], txt: txt))
    }
    func testHintsRejectUnsafeEndpointsAndInvalidPins() {
        for ip in ["192.168.1.3", "172.16.0.3", "10.0.0.5"] { XCTAssertTrue(ConnectionHints.isLanIPv4(ip)) }
        for ip in ["0.0.0.0", "127.0.0.1", "8.8.8.8", "host.local", "192.168.001.3", "999.1.2.3"] { XCTAssertFalse(ConnectionHints.isLanIPv4(ip)) }
        XCTAssertEqual(ConnectionHints.fingerprint(String(repeating: "A", count: 64)), String(repeating: "a", count: 64))
        XCTAssertNil(ConnectionHints.fingerprint(String(repeating: "AA:", count: 31) + "AA"))
        XCTAssertNil(ConnectionHints.fingerprint(String(repeating: "g", count: 64)))
        XCTAssertNil(DiscoveredReceiver(record: ["id": "bad", "port": "oops", "addresses": ["192.168.1.1"]]))
    }
    func testDiscoverySelectsIPv4AndRequiresCompleteIdentity() {
        var record: [String: Any] = ["id": "tv", "name": "客厅._lancast._tcp.local.", "port": 8787, "addresses": ["::1", "192.168.1.9"], "fingerprint": String(repeating: "b", count: 64)]
        let found = DiscoveredReceiver.parse([record, record])
        XCTAssertEqual(found.count, 1); XCTAssertEqual(found[0].name, "客厅")
        XCTAssertEqual(found[0].address, "192.168.1.9:8787")
        record.removeValue(forKey: "fingerprint")
        XCTAssertNil(DiscoveredReceiver(record: record))
        record["addresses"] = ["::1"]
        XCTAssertNil(DiscoveredReceiver(record: record))
    }
}
