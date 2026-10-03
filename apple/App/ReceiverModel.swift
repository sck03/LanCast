import SwiftUI
import AVKit
import LiveKitWebRTC

final class ReceiverModel: ObservableObject {
    struct Approval: Identifiable { let id: String; let name: String; let address: String }
    @Published var address = (localAddresses().first ?? "") + ":8787"
    @Published var status = "选择本机局域网地址，启动接收"
    @Published var fingerprint = ""
    @Published var invite = ""
    @Published var approval: Approval?
    @Published var video: LKRTCVideoTrack?
    @Published var player: AVPlayer?
    @Published var listening = false
    private var core: CoreSession?
    private var rtc: RtcSession?
    private var session: String?
    private var observation: NSKeyValueObservation?
    private var endObserver: NSObjectProtocol?
    private var generation = UUID()

    func start() {
        guard core == nil else { return }
        do {
            #if !os(macOS)
            try AVAudioSession.sharedInstance().setCategory(.playback, mode: .moviePlayback)
            try AVAudioSession.sharedInstance().setActive(true)
            UIApplication.shared.isIdleTimerDisabled = true
            #endif
            core = try CoreSession { [weak self] in self?.event($0) }
            core?.command("listen", ["address": address, "name": "LanCast Apple", "variant": "apple"])
            status = "正在启动安全接收…"
        } catch { status = error.localizedDescription }
    }
    func approve(_ accept: Bool) {
        if let approval { core?.command("approve", ["connectionId": approval.id, "accept": accept]) }
        approval = nil
    }
    func refreshInvite() { core?.command("invite") }
    func stopMedia() {
        core?.command("stop"); clearMedia(); status = "投屏已停止，可更新邀请"
    }
    func stop() {
        clearMedia(); core?.close(); core = nil; listening = false; approval = nil; invite = ""; fingerprint = ""
        #if !os(macOS)
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
        UIApplication.shared.isIdleTimerDisabled = false
        #endif
    }
    private func clearMedia() {
        generation = UUID(); session = nil
        observation = nil; if let endObserver { NotificationCenter.default.removeObserver(endObserver) }; endObserver = nil
        player?.pause(); player?.replaceCurrentItem(with: nil); player = nil
        rtc?.close(); rtc = nil; video = nil
    }
    private func event(_ event: JSONObject) {
        let body = event.object("body")
        switch event.string("type") {
        case "receiver.ready": listening = true; fingerprint = body.string("fingerprint"); invite = body.string("invite"); status = "等待连接；请发送端核对完整指纹"
        case "receiver.invite": invite = body.string("invite")
        case "pair.request":
            // Never overwrite an unanswered approval with another device's request.
            if approval != nil { core?.command("approve", ["connectionId": body.string("connectionId"), "accept": false]) }
            else { approval = Approval(id: body.string("connectionId"), name: body.string("senderName"), address: body.string("address")) }
        case "session.started":
            clearMedia(); session = body.string("sessionId"); let current = generation
            if body.string("mode") == "mirror" {
                rtc = RtcSession(sending: false, audio: true, signal: { [weak self] type, data in
                    DispatchQueue.main.async { guard let self, self.generation == current else { return }; self.core?.send(type, session: self.session, body: data) }
                }, status: { [weak self] value in
                    DispatchQueue.main.async {
                        guard let self, self.generation == current else { return }
                        if value == "first_frame" { self.markReady() }
                        else if value != "rtc_connected" { self.stopMedia(); self.status = value }
                    }
                }, track: { [weak self] track in DispatchQueue.main.async { guard let self, self.generation == current else { return }; self.video = track } })
            }
            status = "正在建立媒体连接…"
        case "message":
            guard body.string("sessionId") == session else { return }
            let data = body.object("body")
            switch body.string("type") {
            case "rtc.offer": rtc?.offer(data)
            case "rtc.ice": rtc?.ice(data)
            case "file.load": core?.command("bridge.create", ["url": data.string("url"), "fingerprint": data.string("fingerprint")])
            case "playback.command":
                switch data.string("action") {
                case "play": player?.play()
                case "pause": player?.pause()
                case "seek": player?.seek(to: CMTime(value: Int64(data["positionMs"] as? Int ?? 0), timescale: 1000))
                case "volume": player?.volume = Float(data["value"] as? Double ?? 1)
                default: break
                }
            case "session.stop": clearMedia(); status = "发送端已停止"
            default: break
            }
        case "bridge.ready":
            guard session != nil, let url = URL(string: body.string("url")), url.host == "127.0.0.1", url.scheme == "http" else { return }
            let current = generation
            let item = AVPlayerItem(url: url); player = AVPlayer(playerItem: item)
            observation = item.observe(\.status, options: [.new, .initial]) { [weak self] item, _ in
                DispatchQueue.main.async {
                    guard let self, self.generation == current else { return }
                    if item.status == .readyToPlay { self.player?.play(); self.markReady() }
                    else if item.status == .failed { self.stopMedia(); self.status = "文件播放失败：\(item.error?.localizedDescription ?? "不支持的媒体")" }
                }
            }
            endObserver = NotificationCenter.default.addObserver(forName: .AVPlayerItemDidPlayToEndTime, object: item, queue: .main) { [weak self] _ in
                guard let self, self.generation == current else { return }; self.stopMedia()
            }
        case "session.closed", "stopped": clearMedia(); status = "会话已结束"
        case "error": status = body.string("code"); if session != nil { stopMedia(); status = body.string("code") }
        default: break
        }
    }
    private func markReady() { status = "正在播放"; core?.send("session.state", session: session, body: ["state": "ready"]) }
    deinit { core?.close(); rtc?.close(); if let endObserver { NotificationCenter.default.removeObserver(endObserver) } }
}
