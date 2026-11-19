import Foundation

/// Refresh never expands the user's capture grant to a different source.
public enum SourceSelection {
    public static func reconcile(selected: String, available: [String]) -> String {
        available.contains(selected) ? selected : ""
    }
}

/// App Group status carries identity and liveness, never capture permission.
public struct BroadcastStatus: Codable, Equatable {
    public enum Phase: String, Codable { case prepared, starting, broadcasting, finished, failed }
    public let id: UUID
    public let phase: Phase
    public let updatedAt: Date
    public init(id: UUID, phase: Phase, now: Date = Date()) {
        self.id = id; self.phase = phase; updatedAt = now
    }
    public func isLive(now: Date = Date()) -> Bool {
        let age = now.timeIntervalSince(updatedAt)
        guard age >= 0 else { return false }
        switch phase {
        case .prepared: return age < 120
        case .starting, .broadcasting: return age < 15
        case .finished, .failed: return false
        }
    }
}
