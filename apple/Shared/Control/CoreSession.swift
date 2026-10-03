import Foundation
import LanCastContracts

typealias JSONObject = [String: Any]
extension Dictionary where Key == String, Value == Any {
    func string(_ key: String) -> String { self[key] as? String ?? "" }
    func object(_ key: String) -> JSONObject { self[key] as? JSONObject ?? [:] }
}

/// Only this adapter touches the C ABI. Poll/free/destroy share one queue.
final class CoreSession {
    private let queue = DispatchQueue(label: "dev.lancast.control")
    private let delivery: DispatchQueue
    private var handle: UInt64 = 0
    private var timer: DispatchSourceTimer?
    private let onEvent: (JSONObject) -> Void
    private let lock = NSLock()
    private var closed = false

    init(delivery: DispatchQueue = .main, onEvent: @escaping (JSONObject) -> Void) throws {
        self.delivery = delivery; self.onEvent = onEvent
        var config = LancastConfig(size: UInt32(MemoryLayout<LancastConfig>.size), abi_version: 2, flags: 0)
        handle = lancast_create_v2(&config)
        guard handle != 0 else { throw CastFailure.invalid("无法创建控制核心") }
        let timer = DispatchSource.makeTimerSource(queue: queue)
        self.timer = timer
        timer.schedule(deadline: .now(), repeating: .milliseconds(20))
        timer.setEventHandler { [weak self] in self?.poll() }
        timer.resume()
    }
    private var isClosed: Bool { lock.lock(); defer { lock.unlock() }; return closed }
    func command(_ op: String, _ body: JSONObject = [:]) {
        var command = body; command["op"] = op
        guard !isClosed, let bytes = try? JSONSerialization.data(withJSONObject: command) else { return }
        queue.async { [weak self] in
            guard let self, !self.isClosed else { return }
            let result = bytes.withUnsafeBytes { lancast_command(self.handle, $0.bindMemory(to: UInt8.self).baseAddress, $0.count) }
            if result != 0 { self.deliver(["type": "error", "body": ["code": "CONTROL_COMMAND_REJECTED"]]) }
        }
    }
    @discardableResult func send(_ type: String, session: String?, body: JSONObject = [:], id: String = UUID().uuidString) -> String {
        command("send", ["message": ["version": 1, "id": id, "type": type, "sessionId": session as Any? ?? NSNull(), "body": body]])
        return id
    }
    private func deliver(_ event: JSONObject) {
        delivery.async { [weak self] in guard let self, !self.isClosed else { return }; self.onEvent(event) }
    }
    private func poll() {
        guard !isClosed else { return }
        for _ in 0..<32 {
            let buffer = lancast_poll(handle)
            guard let pointer = buffer.data else { return }
            let data = Data(bytes: pointer, count: buffer.len)
            lancast_free_buffer(buffer)
            if let event = try? JSONSerialization.jsonObject(with: data) as? JSONObject { deliver(event) }
        }
    }
    func close() {
        lock.lock(); guard !closed else { lock.unlock(); return }; closed = true; lock.unlock()
        let old = handle
        timer?.cancel(); timer = nil
        // Capture only the integer so deinit cannot resurrect self. Never join on UI/RTC threads.
        queue.async { _ = lancast_shutdown(old); lancast_destroy(old) }
    }
    deinit { close() }
}
