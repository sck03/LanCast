import Foundation
import Network

/// Network changes invalidate bound listeners; a new receiver start creates new consent.
final class NetworkMonitor {
    private let monitor = NWPathMonitor()
    init(changed: @escaping () -> Void) {
        monitor.pathUpdateHandler = { _ in DispatchQueue.main.async(execute: changed) }
        monitor.start(queue: DispatchQueue(label: "lancast.network"))
    }
    deinit { monitor.cancel() }
}
