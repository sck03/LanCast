import Foundation
import Darwin
import LanCastContracts

func localAddresses() -> [String] {
    var head: UnsafeMutablePointer<ifaddrs>?
    guard getifaddrs(&head) == 0 else { return [] }
    defer { freeifaddrs(head) }
    var addresses: [(String, String)] = []; var cursor = head
    while let current = cursor {
        defer { cursor = current.pointee.ifa_next }
        guard let address = current.pointee.ifa_addr, address.pointee.sa_family == UInt8(AF_INET),
              current.pointee.ifa_flags & UInt32(IFF_UP) != 0,
              current.pointee.ifa_flags & UInt32(IFF_LOOPBACK) == 0 else { continue }
        let name = String(cString: current.pointee.ifa_name)
        // Wi-Fi/Ethernet and explicit sharing bridges; exclude cellular and VPN tunnels.
        guard name.hasPrefix("en") || name.hasPrefix("bridge") else { continue }
        var host = [CChar](repeating: 0, count: Int(NI_MAXHOST))
        if getnameinfo(address, socklen_t(address.pointee.sa_len), &host, socklen_t(host.count), nil, 0, NI_NUMERICHOST) == 0 {
            let ip = String(cString: host)
            if ConnectionHints.isLanIPv4(ip) { addresses.append((name, ip)) }
        }
    }
    var seen = Set<String>()
    return addresses.sorted { a, b in
        if a.0.hasPrefix("en") != b.0.hasPrefix("en") { return a.0.hasPrefix("en") }
        return a.0 == b.0 ? a.1 < b.1 : a.0 < b.0
    }.map { $0.1 }.filter { seen.insert($0).inserted }
}

/// UDP connect only queries the system route; no payload is sent and no DNS is used.
func localAddressFor(receiver: String, manual: String? = nil) throws -> String {
    let addresses = localAddresses()
    if let manual {
        guard addresses.contains(manual) else { throw CastFailure.invalid("所选网络已断开，请重新自动选择") }
        return manual
    }
    let ip = String(receiver.split(separator: ":").first ?? "")
    guard ConnectionHints.isLanIPv4(ip) else { throw CastFailure.invalid("请选择有效的局域网接收端") }
    let descriptor = socket(AF_INET, SOCK_DGRAM, 0)
    guard descriptor >= 0 else { throw CastFailure.invalid("无法读取本机网络路由") }
    defer { Darwin.close(descriptor) }
    var remote = sockaddr_in()
    remote.sin_len = UInt8(MemoryLayout<sockaddr_in>.size); remote.sin_family = sa_family_t(AF_INET); remote.sin_port = UInt16(9).bigEndian
    guard ip.withCString({ inet_pton(AF_INET, $0, &remote.sin_addr) }) == 1 else { throw CastFailure.invalid("接收端地址无效") }
    let connected = withUnsafePointer(to: &remote) { pointer in
        pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) { Darwin.connect(descriptor, $0, socklen_t(MemoryLayout<sockaddr_in>.size)) }
    }
    var local = sockaddr_in(); var length = socklen_t(MemoryLayout<sockaddr_in>.size)
    let queried = withUnsafeMutablePointer(to: &local) { pointer in
        pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) { getsockname(descriptor, $0, &length) }
    }
    var buffer = [CChar](repeating: 0, count: Int(INET_ADDRSTRLEN))
    guard connected == 0, queried == 0, inet_ntop(AF_INET, &local.sin_addr, &buffer, socklen_t(INET_ADDRSTRLEN)) != nil else { throw CastFailure.invalid("没有到这台电视的局域网路由") }
    let selected = String(cString: buffer)
    guard addresses.contains(selected) else { throw CastFailure.invalid("请连接与电视相同的 Wi-Fi，或在高级设置选择网络") }
    return selected
}
