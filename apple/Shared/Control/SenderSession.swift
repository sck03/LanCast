import Foundation
import CoreMedia
import LiveKitWebRTC
import LanCastContracts

/// Owned by either the Mac application or the broadcast extension, never both.
final class SenderSession {
    private(set) var core: CoreSession?
    private var rtc: RtcSession?
    private var gate = SessionGate()
    private var ticket: BroadcastTicket?
    private var media: JSONObject?
    private var retainedFile: URL?
    private var sharingFile = false
    var status: (String) -> Void = { _ in }
    var readyForCapture: () -> Void = {}
    var ended: () -> Void = {}
    private(set) var connected = false

    func connect(_ ticket: BroadcastTicket, mirror: Bool) throws {
        stop(); try ticket.validate(); self.ticket = ticket
        core = try CoreSession { [weak self] in self?.event($0) }
        media = mirror ? nil : [:]
        core?.command("connect", ["address": ticket.address, "fingerprint": ticket.fingerprint, "invite": ticket.invite, "name": "LanCast Apple"])
        status("等待接收端确认…")
    }
    func shareFile(_ url: URL, localAddress: String) throws {
        guard connected, let ticket, !sharingFile, gate.pending == nil, gate.session == nil else { throw CastFailure.invalid("请先连接接收端并停止当前分享") }
        sharingFile = true
        retainedFile = url
        core?.command("file.share", ["path": url.path, "address": "\(localAddress):0", "allowedIp": String(ticket.address.split(separator: ":")[0]), "encrypted": true])
        status("准备 MP4 文件…")
    }
    private func begin(_ mode: String) {
        let request = UUID().uuidString; gate.begin(request: request)
        core?.send("session.start", session: nil, body: ["mode": mode, "audioRequested": ticket?.audio ?? true], id: request)
    }
    private func event(_ event: JSONObject) {
        let body = event.object("body")
        switch event.string("type") {
        case "connected": connected = true; if media == nil { begin("mirror") } else { status("已连接，可选择 MP4 文件") }
        case "file.shared": media = body; begin("file")
        case "message":
            let data = body.object("body")
            if body.string("type") == "session.accepted" {
                guard gate.accept(reply: body.string("replyTo"), session: body.string("sessionId")) else { return }
                if let media, !media.isEmpty {
                    var load = media; load["mediaId"] = UUID().uuidString; load["durationMs"] = NSNull()
                    core?.send("file.load", session: gate.session, body: load)
                } else {
                    let generation = gate.generation
                    let rtc = RtcSession(sending: true, audio: ticket?.audio ?? false, signal: { [weak self] type, data in
                        DispatchQueue.main.async { guard let self, self.gate.matches(generation) else { return }; self.core?.send(type, session: self.gate.session, body: data) }
                    }, status: { [weak self] value in
                        DispatchQueue.main.async {
                            guard let self, self.gate.matches(generation) else { return }
                            if value == "rtc_connected" { self.status("媒体已连接") } else { self.terminate(value) }
                        }
                    })
                    self.rtc = rtc; rtc.start(profile: data.object("selectedProfile")); readyForCapture()
                }
            } else if body.string("type") == "error" { terminate(data.string("code")) }
            else if body.string("sessionId") == gate.session {
                switch body.string("type") {
                case "rtc.answer": rtc?.answer(data)
                case "rtc.ice": rtc?.ice(data)
                case "session.stop": terminate("接收端已停止")
                case "session.state": status(data.string("state") == "ready" ? "接收端正在播放" : data.string("state"))
                default: break
                }
            }
        case "error": terminate(body.string("code"))
        case "disconnected": terminate("控制连接已断开，请重新授权分享")
        default: break
        }
    }
    func pushVideo(_ sample: CMSampleBuffer, rotation: LKRTCVideoRotation = ._0) { rtc?.pushVideo(sample, rotation: rotation) }
    func pushAudio(_ sample: CMSampleBuffer) { rtc?.pushAudio(sample) }
    func playback(_ action: String, positionMs: Int = 0, volume: Double = 1) {
        core?.send("playback.command", session: gate.session, body: ["action": action, "positionMs": max(0, positionMs), "value": min(1, max(0, volume))])
    }
    private func terminate(_ value: String) { stop(); status(value); ended() }
    func stop() {
        // Closing the authenticated connection revokes its remote session, including pending starts.
        gate.stop(); rtc?.close(); rtc = nil; core?.close(); core = nil
        connected = false; ticket = nil; media = nil; sharingFile = false
        retainedFile?.stopAccessingSecurityScopedResource(); retainedFile = nil
    }
    deinit { stop() }
}
