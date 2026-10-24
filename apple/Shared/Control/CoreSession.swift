import Foundation
import LanCastContracts

typealias JSONObject = [String: Any]
extension Dictionary where Key == String, Value == Any {
    func string(_ key: String) -> String { self[key] as? String ?? "" }
    func object(_ key: String) -> JSONObject { self[key] as? JSONObject ?? [:] }
}

/// Lifetime shared with the waiter, without retaining CoreSession or its owner.
private final class ControlHandle {
    let value: UInt64
    let deliverySlot = DispatchSemaphore(value: 1)
    private let lock = NSLock()
    private var closed = false
    init() throws {
        var config = LancastConfig(size: UInt32(MemoryLayout<LancastConfig>.size), abi_version: 2, flags: 0)
        value = lancast_create_v2(&config)
        guard value != 0 else { throw CastFailure.invalid("无法创建控制核心") }
    }
    var isClosed: Bool { lock.lock(); defer { lock.unlock() }; return closed }
    @discardableResult func close() -> Bool {
        lock.lock()
        guard !closed else { lock.unlock(); return false }
        closed = true
        lock.unlock()
        _ = lancast_shutdown(value)
        deliverySlot.signal()
        return true
    }
}

/// Only this adapter touches the C ABI. One worker waits/frees/destroys; UI delivery is bounded.
final class CoreSession {
    private let state: ControlHandle
    private let delivery: DispatchQueue
    private let onEvent: (JSONObject) -> Void
    private let commandErrorSlot = DispatchSemaphore(value: 1)

    init(delivery: DispatchQueue = .main, onEvent: @escaping (JSONObject) -> Void) throws {
        let state = try ControlHandle()
        self.state = state; self.delivery = delivery; self.onEvent = onEvent
        DispatchQueue(label: "dev.lancast.control").async {
            defer { lancast_destroy(state.value) }
            while !state.isClosed {
                state.deliverySlot.wait()
                if state.isClosed { break }
                let first = lancast_wait_poll(state.value, 1000)
                guard first.data != nil else { state.deliverySlot.signal(); continue }
                var batch: [JSONObject] = []
                if let event = Self.decode(first) { batch.append(event) }
                for _ in 1..<32 {
                    let buffer = lancast_poll(state.value)
                    guard buffer.data != nil else { break }
                    if let event = Self.decode(buffer) { batch.append(event) }
                }
                let events = batch
                delivery.async {
                    defer { state.deliverySlot.signal() }
                    for event in events {
                        guard !state.isClosed else { break }
                        onEvent(event)
                    }
                }
            }
        }
    }
    private static func decode(_ buffer: LancastBuffer) -> JSONObject? {
        guard let pointer = buffer.data else { return nil }
        defer { lancast_free_buffer(buffer) }
        let data = Data(bytes: pointer, count: buffer.len)
        return (try? JSONSerialization.jsonObject(with: data)) as? JSONObject
    }
    func command(_ op: String, _ body: JSONObject = [:]) {
        var command = body; command["op"] = op
        guard !state.isClosed, let bytes = try? JSONSerialization.data(withJSONObject: command) else { return }
        // Registry handles are safe across concurrent shutdown. This only enqueues work.
        let result = bytes.withUnsafeBytes { lancast_command(state.value, $0.bindMemory(to: UInt8.self).baseAddress, $0.count) }
        if result != 0 && commandErrorSlot.wait(timeout: .now()) == .success {
            let slot = commandErrorSlot
            delivery.async { [weak self] in
                defer { slot.signal() }
                guard let self, !self.state.isClosed else { return }
                self.onEvent(["type": "error", "body": ["code": "CONTROL_COMMAND_REJECTED", "terminal": true]])
            }
        }
    }
    @discardableResult func send(_ type: String, session: String?, body: JSONObject = [:], id: String = UUID().uuidString) -> String {
        command("send", ["message": ["version": 1, "id": id, "type": type, "sessionId": session as Any? ?? NSNull(), "body": body]])
        return id
    }
    func close() { state.close() }
    deinit { close() }
}
