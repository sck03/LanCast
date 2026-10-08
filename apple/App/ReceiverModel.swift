import SwiftUI
import AVKit
import LiveKitWebRTC
import LanCastContracts

final class ReceiverModel: ObservableObject {
    struct Approval: Identifiable { let id: String; let name: String; let address: String }
    @Published var address = (localAddresses().first ?? "") + ":8787"
    @Published var availableNetworks = localAddresses()
    @Published var networkChoice = ""
    @Published var manualAddress = false
    @Published var status = "本机网络会自动选择，点击启动接收即可"
    @Published var fingerprint = ""
    @Published var invite = ""
    @Published var approval: Approval?
    // SwiftUI may clear the alert binding before invoking its captured button action.
    private var pendingApprovalID: String?
    @Published var video: LKRTCVideoTrack?
    @Published var player: AVPlayer?
    @Published var listening = false
    private var core: CoreSession?
    private var fileBridge: CoreSession?
    private var rtc: RtcSession?
    private var session: String?
    private var negotiation: String?
    private var observation: NSKeyValueObservation?
    private var endObserver: NSObjectProtocol?
    private var generation = UUID()

    func start() {
        guard core == nil else { return }
        do {
            refreshNetwork()
            let parts = address.split(separator: ":", omittingEmptySubsequences: false)
            guard parts.count == 2, ConnectionHints.isLanIPv4(String(parts[0])), let port = UInt16(parts[1]), port > 0 else {
                throw CastFailure.invalid("请连接本地网络，或在网络设置选择有效地址")
            }
            #if !os(macOS)
            try AVAudioSession.sharedInstance().setCategory(.playback, mode: .moviePlayback)
            try AVAudioSession.sharedInstance().setActive(true)
            UIApplication.shared.isIdleTimerDisabled = true
            #endif
            core = try CoreSession { [weak self] in self?.event($0) }
            core?.command("listen", ["address": address, "name": "LanCast Apple", "variant": "apple"])
            status = "正在启动安全接收…"
        } catch { stop(); status = error.localizedDescription }
    }
    func refreshNetwork() {
        guard core == nil else { return }
        availableNetworks = localAddresses()
        if !availableNetworks.contains(networkChoice) { networkChoice = "" }
        if !manualAddress {
            let ip = networkChoice.isEmpty ? availableNetworks.first ?? "" : networkChoice
            address = ip.isEmpty ? "" : "\(ip):8787"
        }
    }
    func approve(_ accept: Bool, connection: String? = nil) {
        guard let id = connection ?? pendingApprovalID, id == pendingApprovalID else { return }
        core?.command("approve", ["connectionId": id, "accept": accept])
        pendingApprovalID = nil; approval = nil
    }
    func refreshInvite() { core?.command("invite") }
    func stopMedia() {
        core?.command("stop"); clearMedia(); status = "投屏已停止，可刷新配对码"
    }
    func stop() {
        clearMedia(); core?.close(); core = nil; listening = false; pendingApprovalID = nil; approval = nil; invite = ""; fingerprint = ""
        #if !os(macOS)
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
        UIApplication.shared.isIdleTimerDisabled = false
        #endif
    }
    private func clearMedia() {
        generation = UUID(); session = nil; negotiation = nil
        clearPlayer()
        rtc?.close(); rtc = nil; video = nil
    }
    private func clearPlayer() {
        observation = nil; if let endObserver { NotificationCenter.default.removeObserver(endObserver) }; endObserver = nil
        player?.pause(); player?.replaceCurrentItem(with: nil); player = nil
        fileBridge?.close(); fileBridge = nil
    }
    private func event(_ event: JSONObject) {
        let body = event.object("body")
        switch event.string("type") {
        case "receiver.ready": listening = true; address = body.string("address"); fingerprint = body.string("fingerprint"); invite = body.string("invite"); status = "在发送端选择此设备并输入配对码，再在本屏允许连接"
        case "receiver.invite": invite = body.string("invite")
        case "pair.request":
            // Never overwrite an unanswered approval with another device's request.
            if pendingApprovalID != nil { core?.command("approve", ["connectionId": body.string("connectionId"), "accept": false]) }
            else {
                pendingApprovalID = body.string("connectionId")
                approval = Approval(id: body.string("connectionId"), name: body.string("senderName"), address: body.string("address"))
            }
        case "pair.closed":
            if pendingApprovalID == body.string("connectionId") { pendingApprovalID = nil; approval = nil }
        case "session.started":
            clearMedia(); session = body.string("sessionId"); let current = generation
            if body.string("mode") == "mirror" {
                rtc = RtcSession(sending: false, audio: true, signal: { [weak self] type, data in
                    DispatchQueue.main.async { guard let self, self.generation == current else { return }; self.core?.send(type, session: self.session, body: data) }
                }, status: { [weak self] value in
                    DispatchQueue.main.async {
                        guard let self, self.generation == current else { return }
                        if value == "first_frame" { self.markReady() }
                        else if value == "rtc_reconnecting" { self.status = "媒体连接中断，正在恢复…" }
                        else if value != "rtc_connected" { self.stopMedia(); self.status = value }
                    }
                }, track: { [weak self] track in DispatchQueue.main.async { guard let self, self.generation == current else { return }; self.video = track } }, recoveryEnabled: body.string("rtcRecovery") == "replace-v1")
            }
            status = "正在建立媒体连接…"
        case "message":
            guard body.string("sessionId") == session else { return }
            let data = body.object("body")
            switch body.string("type") {
            case "rtc.offer": negotiation = data.string("negotiationId"); rtc?.offer(data)
            case "rtc.ice": rtc?.ice(data)
            case "file.load": prepareFile(data)
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
        case "session.closed": clearMedia(); status = "会话已结束"
        case "error":
            if !listening { stop() } else if session != nil { stopMedia() }
            status = body.string("code")
        default: break
        }
    }
    private func prepareFile(_ data: JSONObject) {
        clearPlayer(); generation = UUID()
        let current = generation
        do {
            fileBridge = try CoreSession { [weak self] event in
                guard let self, self.generation == current else { return }
                self.fileEvent(event)
            }
            fileBridge?.command("bridge.create", ["url": data.string("url"), "fingerprint": data.string("fingerprint")])
            DispatchQueue.main.asyncAfter(deadline: .now() + 20) { [weak self] in
                guard let self, self.generation == current, self.player?.currentItem?.status != .readyToPlay else { return }
                self.stopMedia(); self.status = "文件加载超时，请检查发送端与媒体格式"
            }
        } catch { stopMedia(); status = error.localizedDescription }
    }
    private func fileEvent(_ event: JSONObject) {
        let body = event.object("body")
        switch event.string("type") {
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
        case "error": stopMedia(); status = body.string("code")
        default: break
        }
    }
    private func markReady() {
        status = "正在播放"
        var body: JSONObject = ["state": "ready"]
        if let negotiation { body["negotiationId"] = negotiation }
        core?.send("session.state", session: session, body: body)
    }
    deinit { core?.close(); fileBridge?.close(); rtc?.close(); if let endObserver { NotificationCenter.default.removeObserver(endObserver) } }
}
