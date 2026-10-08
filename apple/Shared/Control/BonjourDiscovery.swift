import Foundation
import Darwin
import LanCastContracts

/// System Bonjour performs multicast on our behalf; never request a raw multicast entitlement.
final class ReceiverDiscovery: NSObject, NetServiceBrowserDelegate, NetServiceDelegate {
    private let browser = NetServiceBrowser()
    private var services: [String: NetService] = [:]
    private var found: [String: DiscoveredReceiver] = [:]
    private var timeout: DispatchWorkItem?
    private var active = false
    private let completion: (Result<[DiscoveredReceiver], Error>) -> Void
    init(completion: @escaping (Result<[DiscoveredReceiver], Error>) -> Void) { self.completion = completion; super.init() }
    func start() {
        active = true; browser.delegate = self
        browser.searchForServices(ofType: "_lancast._tcp.", inDomain: "local.")
        let task = DispatchWorkItem { [weak self] in self?.finish() }
        timeout = task; DispatchQueue.main.asyncAfter(deadline: .now() + 5, execute: task)
    }
    func stop() {
        active = false; timeout?.cancel(); timeout = nil
        browser.delegate = nil; browser.stop()
        for service in services.values { service.delegate = nil; service.stop() }
        services.removeAll(); found.removeAll()
    }
    private func key(_ service: NetService) -> String { "\(service.name).\(service.type)\(service.domain)" }
    func netServiceBrowser(_ browser: NetServiceBrowser, didFind service: NetService, moreComing: Bool) {
        let id = key(service)
        guard active, services.count < 64 || services[id] != nil else { return }
        services[id]?.delegate = nil; services[id]?.stop()
        services[id] = service; service.delegate = self; service.resolve(withTimeout: 4)
    }
    func netServiceBrowser(_ browser: NetServiceBrowser, didRemove service: NetService, moreComing: Bool) {
        let id = key(service)
        guard active, services[id] === service else { return }
        services[id]?.delegate = nil; services[id]?.stop(); services.removeValue(forKey: id); found.removeValue(forKey: id)
    }
    func netServiceBrowser(_ browser: NetServiceBrowser, didNotSearch errorDict: [String: NSNumber]) {
        guard active else { return }; stop()
        completion(.failure(CastFailure.invalid("搜索失败，请检查本地网络权限和Wi-Fi连接")))
    }
    func netServiceDidResolveAddress(_ service: NetService) {
        guard active, services[key(service)] === service, let data = service.txtRecordData() else { return }
        let addresses = (service.addresses ?? []).compactMap { data -> String? in
            data.withUnsafeBytes { raw -> String? in
                guard raw.count >= MemoryLayout<sockaddr_in>.size else { return nil }
                var address = raw.loadUnaligned(as: sockaddr_in.self)
                guard address.sin_family == sa_family_t(AF_INET) else { return nil }
                var text = [CChar](repeating: 0, count: Int(INET_ADDRSTRLEN))
                guard inet_ntop(AF_INET, &address.sin_addr, &text, socklen_t(INET_ADDRSTRLEN)) != nil else { return nil }
                return String(cString: text)
            }
        }
        found[key(service)] = DiscoveredReceiver.bonjour(name: service.name, port: service.port,
            resolved: addresses, txt: NetService.dictionary(fromTXTRecord: data))
    }
    private func finish() {
        guard active else { return }
        var seen = Set<String>()
        let result = found.values.sorted { $0.id < $1.id }.filter { seen.insert($0.id).inserted }
        stop(); completion(.success(result))
    }
    deinit { stop() }
}

/// Publishes the Rust WSS listener's exact bound address and public identity.
final class ReceiverAdvertisement: NSObject, NetServiceDelegate {
    private let service: NetService
    private let state: (Result<Void, Error>) -> Void
    private var active = false
    init(address: String, deviceID: String, fingerprint: String, state: @escaping (Result<Void, Error>) -> Void) throws {
        let parts = address.split(separator: ":", omittingEmptySubsequences: false)
        guard parts.count == 2, ConnectionHints.isLanIPv4(String(parts[0])), let port = UInt16(parts[1]), port > 0,
              ConnectionHints.fingerprint(fingerprint) != nil else { throw CastFailure.invalid("接收服务信息无效") }
        service = NetService(domain: "local.", type: "_lancast._tcp.", name: "LanCast Apple", port: Int32(port))
        self.state = state; super.init(); service.delegate = self
        let txt = ["version": "1", "pairingVersion": "2", "deviceId": deviceID, "variant": "apple",
                   "address": String(parts[0]), "fingerprint": fingerprint].mapValues { Data($0.utf8) }
        guard service.setTXTRecord(NetService.data(fromTXTRecord: txt)) else { throw CastFailure.invalid("无法发布接收服务信息") }
    }
    func start() { active = true; service.publish() }
    func stop() { active = false; service.delegate = nil; service.stop() }
    func netServiceDidPublish(_ sender: NetService) { if active { state(.success(())) } }
    func netService(_ sender: NetService, didNotPublish errorDict: [String: NSNumber]) {
        guard active else { return }; stop(); state(.failure(CastFailure.invalid("无法发布接收服务，请检查本地网络权限")))
    }
    deinit { stop() }
}
