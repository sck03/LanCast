import Foundation

public enum ConnectionHints {
    public static func isLanIPv4(_ value: String) -> Bool {
        let fields = value.split(separator: ".", omittingEmptySubsequences: false)
        guard fields.count == 4 else { return false }
        var bytes: [UInt8] = []
        for field in fields {
            guard !field.isEmpty, field.count <= 3, field.allSatisfy({ "0123456789".contains($0) }),
                  field.count == 1 || field.first != "0", let byte = UInt8(field) else { return false }
            bytes.append(byte)
        }
        return bytes[0] == 10 || (bytes[0] == 172 && (16...31).contains(bytes[1])) ||
            (bytes[0] == 192 && bytes[1] == 168) || (bytes[0] == 169 && bytes[1] == 254)
    }
    public static func fingerprint(_ value: String) -> String? {
        let pin = value.filter { !$0.isWhitespace && $0 != ":" }.lowercased()
        return pin.count == 64 && pin.allSatisfy({ "0123456789abcdef".contains($0) }) ? pin : nil
    }
    public static func displayFingerprint(_ value: String) -> String {
        let chars = Array(value)
        return stride(from: 0, to: chars.count, by: 8).map { index in
            String(chars[index..<min(index + 8, chars.count)]) + (index == 24 ? "\n" : "  ")
        }.joined()
    }
}

/// Untrusted discovery hints. Pairing still requires an out-of-band comparison.
public struct DiscoveredReceiver: Identifiable, Equatable {
    public let id: String
    public let name: String
    public let address: String
    public let fingerprint: String
    public init?(record: [String: Any]) {
        guard let id = record["id"] as? String, !id.isEmpty, id.utf8.count <= 512,
              let port = record["port"] as? Int, (1...65535).contains(port),
              let addresses = record["addresses"] as? [String],
              let ip = addresses.prefix(32).first(where: ConnectionHints.isLanIPv4) else { return nil }
        self.id = id
        let rawName = record["name"] as? String ?? "TV"
        let suffix = "._lancast._tcp.local."
        let name = rawName.hasSuffix(suffix) ? String(rawName.dropLast(suffix.count)) : rawName
        self.name = String(name.unicodeScalars.filter { !CharacterSet.controlCharacters.contains($0) }.prefix(100).map(String.init).joined())
        address = "\(ip):\(port)"
        let rawPin = record["fingerprint"] as? String ?? ""
        fingerprint = rawPin.count == 64 ? ConnectionHints.fingerprint(rawPin) ?? "" : ""
    }
    public static func parse(_ records: [[String: Any]]) -> [DiscoveredReceiver] {
        var seen = Set<String>()
        return records.prefix(128).compactMap { Self(record: $0) }.filter { seen.insert($0.id).inserted }.sorted { $0.id < $1.id }
    }
}
