#if os(iOS)
import Foundation
import LanCastContracts

enum BroadcastStore {
    static var group: String { Bundle.main.object(forInfoDictionaryKey: "LanCastAppGroup") as? String ?? "group.dev.lancast.shared" }
    static func location() throws -> URL {
        guard let directory = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: group) else {
            throw CastFailure.invalid("App Group 未配置，请使用同一团队签名安装应用和扩展")
        }
        return directory.appendingPathComponent("broadcast-ticket.json")
    }
    static func save(_ ticket: BroadcastTicket) throws {
        let url = try location()
        try JSONEncoder().encode(ticket).write(to: url, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
    }
    static func consume() throws -> BroadcastTicket {
        let url = try location()
        // Atomic rename claims a ticket exactly once even if another extension starts concurrently.
        let claim = url.deletingLastPathComponent().appendingPathComponent("ticket-\(UUID().uuidString).json")
        try FileManager.default.moveItem(at: url, to: claim)
        defer { try? FileManager.default.removeItem(at: claim) }
        let bytes = try Data(contentsOf: claim)
        guard bytes.count <= 4096 else { throw CastFailure.invalid("广播配置无效") }
        let ticket = try JSONDecoder().decode(BroadcastTicket.self, from: bytes)
        try ticket.validate(); return ticket
    }
    static func clear() { if let url = try? location() { try? FileManager.default.removeItem(at: url) } }
}
#endif
