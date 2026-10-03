import Foundation

public enum CastFailure: Error, LocalizedError {
    case invalid(String)
    public var errorDescription: String? { if case .invalid(let reason) = self { return reason }; return nil }
}

/// Shared by host and extension; contains no capture grant or persistent trust.
public struct BroadcastTicket: Codable, Equatable {
    public let version: Int
    public let id: UUID
    public let address: String
    public let fingerprint: String
    public let invite: String
    public let audio: Bool
    public let createdAt: Date
    public init(address: String, fingerprint: String, invite: String, audio: Bool, now: Date = Date()) throws {
        version = 1; id = UUID(); self.address = address
        self.fingerprint = fingerprint.lowercased(); self.invite = invite; self.audio = audio; createdAt = now
        try validate(now: now)
    }
    public func validate(now: Date = Date()) throws {
        let parts = address.split(separator: ":", omittingEmptySubsequences: false)
        guard version == 1, parts.count == 2, let port = UInt16(parts[1]), port > 0 else {
            throw CastFailure.invalid("请输入接收端 IPv4:端口")
        }
        let octets = parts[0].split(separator: ".", omittingEmptySubsequences: false)
        guard octets.count == 4, octets.allSatisfy({ UInt8($0) != nil }), parts[0] != "0.0.0.0" else {
            throw CastFailure.invalid("接收端地址无效")
        }
        guard fingerprint.count == 64, fingerprint.allSatisfy({ "0123456789abcdef".contains($0) }),
              !invite.isEmpty, invite.utf8.count <= 256 else { throw CastFailure.invalid("需要完整 SHA-256 指纹和邀请") }
        guard now >= createdAt, now.timeIntervalSince(createdAt) < 120 else { throw CastFailure.invalid("广播配置已过期，请更新接收端邀请后重新准备") }
    }
}

/// Rejects delayed session replies and media callbacks after stop/replacement.
public struct SessionGate {
    public private(set) var generation: UInt64 = 0
    public private(set) var pending: String?
    public private(set) var session: String?
    public init() {}
    public mutating func begin(request: String) { generation &+= 1; pending = request; session = nil }
    public mutating func accept(reply: String, session: String) -> Bool {
        guard pending == reply, UUID(uuidString: session) != nil else { return false }
        pending = nil; self.session = session; return true
    }
    public mutating func stop() { generation &+= 1; pending = nil; session = nil }
    public func matches(_ generation: UInt64) -> Bool { self.generation == generation }
}
