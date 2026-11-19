#if os(iOS)
import Foundation
import LanCastContracts

/// Credentials are one-use; status and cancellation are isolated by ticket ID.
enum BroadcastStore {
    static var group: String { Bundle.main.object(forInfoDictionaryKey: "LanCastAppGroup") as? String ?? "group.dev.lancast.shared" }
    private static func location(_ name: String) throws -> URL {
        guard let directory = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: group) else {
            throw CastFailure.invalid("App Group 未配置，请使用同一团队签名安装应用和扩展")
        }
        return directory.appendingPathComponent(name)
    }
    private static func write<T: Encodable>(_ value: T, _ name: String) throws {
        try JSONEncoder().encode(value).write(to: location(name), options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
    }
    private static func read<T: Decodable>(_ type: T.Type, _ name: String) throws -> T {
        let data = try Data(contentsOf: location(name))
        guard data.count <= 4096 else { throw CastFailure.invalid("广播配置无效") }
        return try JSONDecoder().decode(type, from: data)
    }
    static func current() -> BroadcastStatus? {
        guard let id = try? read(UUID.self, "broadcast-current.json") else { return nil }
        return try? read(BroadcastStatus.self, "broadcast-state-\(id).json")
    }
    static func save(_ ticket: BroadcastTicket) throws {
        if let previous = current() {
            guard !previous.isLive() else { throw CastFailure.invalid("请先停止当前系统广播") }
            for name in ["broadcast-state-\(previous.id).json", "broadcast-stop-\(previous.id).json"] {
                if let url = try? location(name) { try? FileManager.default.removeItem(at: url) }
            }
        }
        try publish(ticket.id, phase: .prepared)
        try write(ticket.id, "broadcast-current.json")
        try write(ticket, "broadcast-ticket.json")
    }
    static func consume() throws -> BroadcastTicket {
        let url = try location("broadcast-ticket.json")
        let claim = try location("ticket-\(UUID()).json")
        try FileManager.default.moveItem(at: url, to: claim)
        defer { try? FileManager.default.removeItem(at: claim) }
        let data = try Data(contentsOf: claim)
        guard data.count <= 4096 else { throw CastFailure.invalid("广播配置无效") }
        let ticket = try JSONDecoder().decode(BroadcastTicket.self, from: data)
        do {
            try ticket.validate()
            guard !cancelled(ticket.id) else { throw CastFailure.invalid("广播已取消，请重新准备") }
            try publish(ticket.id, phase: .starting)
            return ticket
        } catch {
            try? publish(ticket.id, phase: .failed)
            throw error
        }
    }
    static func publish(_ id: UUID, phase: BroadcastStatus.Phase) throws {
        try write(BroadcastStatus(id: id, phase: phase), "broadcast-state-\(id).json")
    }
    static func cancelled(_ id: UUID) -> Bool {
        (try? read(UUID.self, "broadcast-current.json")) != id ||
            (try? read(UUID.self, "broadcast-stop-\(id).json")) == id
    }
    static func clear() {
        if let state = current() {
            try? write(state.id, "broadcast-stop-\(state.id).json")
            if state.phase == .prepared { try? publish(state.id, phase: .finished) }
        }
        if let url = try? location("broadcast-ticket.json") { try? FileManager.default.removeItem(at: url) }
    }
}
#endif
