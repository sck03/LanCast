#if !os(tvOS)
import SwiftUI
import LanCastContracts

final class SenderModel: ObservableObject {
    @Published private(set) var address = ""
    @Published private(set) var fingerprint = ""
    @Published var invite = "" { didSet { if invite != oldValue { invalidatePreparation() } } }
    @Published var audio = true { didSet { if audio != oldValue { invalidatePreparation() } } }
    @Published var status = "选择电视，地址和指纹会自动填入"
    @Published var localAddress = localAddresses().first ?? ""
    @Published var availableNetworks = localAddresses()
    @Published var networkChoice = "" // Empty means automatic.
    @Published var advanced = false
    @Published var connected = false
    @Published var connecting = false
    @Published var scanning = false
    @Published var broadcastPrepared = false
    @Published var fileConnection = false
    @Published var fileShared = false
    @Published var devices: [DiscoveredReceiver] = []
    @Published var selectedDevice = ""
    enum PairingAction: Equatable { case file, mirror, broadcast }
    struct PairingConfirmation: Identifiable {
        let id = UUID()
        let ticket: BroadcastTicket
        let action: PairingAction
        let source: String?
    }
    @Published var confirmation: PairingConfirmation?
    private var pendingConfirmation: UUID?
    var connectionLocked: Bool { connected || connecting || scanning || broadcastPrepared || confirmation != nil }
    @Published var position = 0.0
    @Published var volume = 1.0
    #if os(macOS)
    @Published var sources: [MacCapture.Source] = []
    @Published var selectedSource = ""
    private let capture = MacCapture()
    #endif
    private let sender = SenderSession()
    private var discovery: ReceiverDiscovery?
    private var generation = UUID()
    init() {
        sender.status = { [weak self] value in self?.status = value; self?.connected = self?.sender.connected ?? false; self?.connecting = self?.sender.connecting ?? false }
        sender.ended = { [weak self] in self?.stopCapture() }
    }
    private func ticket() throws -> BroadcastTicket {
        let endpoint = address.trimmingCharacters(in: .whitespacesAndNewlines)
        guard ConnectionHints.isLanIPv4(String(endpoint.split(separator: ":").first ?? "")), let pin = ConnectionHints.fingerprint(fingerprint) else {
            throw CastFailure.invalid("请搜索并选择当前版本的接收端")
        }
        return try BroadcastTicket(address: endpoint, fingerprint: pin, invite: invite.filter { !$0.isWhitespace }, audio: audio)
    }
    func refreshNetwork() {
        guard !connected, !connecting else { return }
        availableNetworks = localAddresses()
        if !availableNetworks.contains(networkChoice) { networkChoice = "" }
        localAddress = networkChoice.isEmpty ? availableNetworks.first ?? "" : networkChoice
    }
    func selectDevice(_ device: DiscoveredReceiver) {
        guard !connectionLocked else { return }
        invalidatePreparation(); address = device.address; fingerprint = device.fingerprint; invite = ""; selectedDevice = device.id
        status = "已获取地址和指纹，请输入电视上的配对码"
        do { localAddress = try localAddressFor(receiver: device.address, manual: networkChoice.isEmpty ? nil : networkChoice) }
        catch { status = error.localizedDescription }
    }
    private func invalidatePreparation() {
        pendingConfirmation = nil; confirmation = nil
        if broadcastPrepared {
            broadcastPrepared = false
            #if os(iOS)
            BroadcastStore.clear()
            #endif
        }
    }
    func cancelConfirmation() { pendingConfirmation = nil; confirmation = nil }
    private func requestPairing(_ action: PairingAction) {
        guard !connectionLocked else { status = "请先停止当前分享或等待搜索结束"; return }
        do {
            let value = try ticket()
            localAddress = try localAddressFor(receiver: value.address, manual: networkChoice.isEmpty ? nil : networkChoice)
            #if os(macOS)
            if action == .mirror && !sources.contains(where: { $0.id == selectedSource }) { throw CastFailure.invalid("请先选择屏幕或窗口") }
            let source: String? = selectedSource
            #else
            let source: String? = nil
            #endif
            let request = PairingConfirmation(ticket: value, action: action, source: source)
            pendingConfirmation = request.id; confirmation = request
        } catch { status = error.localizedDescription }
    }
    func confirm(_ request: PairingConfirmation) {
        guard pendingConfirmation == request.id else { return }
        cancelConfirmation(); invite = ""
        do {
            try request.ticket.validate()
            switch request.action {
            case .file: stopCapture(); fileConnection = true; fileShared = false; try sender.connect(request.ticket, mirror: false)
            case .mirror:
                #if os(macOS)
                startMirror(request.ticket, sourceID: request.source ?? "")
                #else
                throw CastFailure.invalid("此平台请使用系统屏幕广播")
                #endif
            case .broadcast:
                #if os(iOS)
                sender.stop(); try BroadcastStore.save(request.ticket); broadcastPrepared = true
                status = "2 分钟内点击系统广播按钮，并在电视允许连接；请保留系统录屏授权步骤。"
                #else
                throw CastFailure.invalid("此平台请使用屏幕或窗口分享")
                #endif
            }
        } catch { status = error.localizedDescription }
    }
    func scan() {
        guard !connectionLocked else { return }
        refreshNetwork(); discovery?.stop(); devices = []; selectedDevice = ""; address = ""; fingerprint = ""; invite = ""; scanning = true
        discovery = ReceiverDiscovery { [weak self] result in
            guard let self else { return }
            self.scanning = false; self.discovery = nil
            switch result {
            case .success(let devices):
                self.devices = devices
                self.status = devices.isEmpty ? "未发现接收端，请确认各端为当前版本并连接同一网络；网络设置可切换网卡" : "请选择电视，将自动获取地址和指纹"
                if devices.count == 1 { self.selectDevice(devices[0]) }
            case .failure(let error): self.status = error.localizedDescription
            }
        }
        discovery?.start()
    }
    func connectFile() {
        requestPairing(.file)
    }
    func shareFile(_ result: Result<[URL], Error>) {
        do {
            guard let url = try result.get().first else { return }
            localAddress = try localAddressFor(receiver: address, manual: networkChoice.isEmpty ? nil : networkChoice)
            _ = url.startAccessingSecurityScopedResource()
            do { try sender.shareFile(url, localAddress: localAddress); fileShared = true }
            catch { url.stopAccessingSecurityScopedResource(); throw error }
        } catch { status = error.localizedDescription }
    }
    func playback(_ action: String) {
        guard position.isFinite, position >= 0, position <= 31_536_000, volume.isFinite else {
            status = "请输入有效的播放位置与音量"; return
        }
        sender.playback(action, positionMs: Int(position * 1000), volume: volume)
    }
    #if os(iOS)
    func prepareBroadcast() {
        requestPairing(.broadcast)
    }
    #endif
    #if os(macOS)
    func refreshSources() {
        Task { @MainActor in
            do { sources = try await MacCapture.sources(); selectedSource = sources.first?.id ?? "" }
            catch { status = "读取来源失败，请检查系统屏幕录制权限：\(error.localizedDescription)" }
        }
    }
    func startMirror() {
        requestPairing(.mirror)
    }
    private func startMirror(_ ticket: BroadcastTicket, sourceID: String) {
        guard let source = sources.first(where: { $0.id == sourceID }) else { status = "请先选择窗口或显示器"; return }
        stopCapture()
        let current = UUID(); generation = current
        let wantsAudio = ticket.audio
        capture.video = { [weak self] in self?.sender.pushVideo($0) }
        capture.audio = { [weak self] in self?.sender.pushAudio($0) }
        capture.failed = { [weak self] value in self?.stop(); self?.status = value }
        sender.readyForCapture = { [weak self] in
            Task { @MainActor in
                guard let self, self.generation == current else { return }
                do { try await self.capture.start(source: source, audio: wantsAudio) }
                catch {
                    guard self.generation == current else { return }
                    self.stop(); self.status = "采集失败：\(error.localizedDescription)"
                }
            }
        }
        do { try sender.connect(ticket, mirror: true) } catch { status = error.localizedDescription }
    }
    #endif
    private func stopCapture() {
        generation = UUID(); connected = false; connecting = false; fileConnection = false; fileShared = false
        #if os(macOS)
        Task { await capture.stop() }
        #endif
    }
    func stop() {
        stopCapture(); sender.stop(); discovery?.stop(); discovery = nil; scanning = false; cancelConfirmation(); broadcastPrepared = false
        #if os(iOS)
        BroadcastStore.clear()
        #endif
    }
    func suspendHost() { stopCapture(); sender.stop(); discovery?.stop(); discovery = nil; scanning = false; cancelConfirmation() }
}
#endif
