import Foundation

func localAddresses() -> [String] {
    var head: UnsafeMutablePointer<ifaddrs>?
    guard getifaddrs(&head) == 0 else { return [] }
    defer { freeifaddrs(head) }
    var addresses = Set<String>(); var cursor = head
    while let current = cursor {
        defer { cursor = current.pointee.ifa_next }
        guard let address = current.pointee.ifa_addr, address.pointee.sa_family == UInt8(AF_INET),
              current.pointee.ifa_flags & UInt32(IFF_UP) != 0,
              current.pointee.ifa_flags & UInt32(IFF_LOOPBACK) == 0 else { continue }
        var host = [CChar](repeating: 0, count: Int(NI_MAXHOST))
        if getnameinfo(address, socklen_t(address.pointee.sa_len), &host, socklen_t(host.count), nil, 0, NI_NUMERICHOST) == 0 {
            addresses.insert(String(cString: host))
        }
    }
    return addresses.sorted()
}
