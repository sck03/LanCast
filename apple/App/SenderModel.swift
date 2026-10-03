#if !os(tvOS)
import SwiftUI
import LanCastContracts

final class SenderModel: ObservableObject {
    @Published var address = ""
    @Published var fingerprint = ""
    @Published var invite = ""
    @Published var audio = true
    @Published var status = "输入接收端地址、完整指纹和一次性邀请"
    @Published var localAddress = localAddresses().first ?? ""
    @Published var connected = false
    @Published var broadcastPrepared = false
    @Published var devices: [String] = []
    @Published var position = 0.0
    @Published var volume = 1.0
    #if os(macOS)
    @Published var sources: [MacCapture.Source] = []
    @Published var selectedSource = ""
    private let capture = MacCapture()
    #endif
    private let sender = SenderSession()
    private var discovery: CoreSession?
    private var generation = UUID()
    init() {
        sender.status = { [weak self] value in self?.status = value; self?.connected = self?.sender.connected ?? false }
        sender.ended = { [weak self] in self?.stopCapture() }
    }
    private func ticket() throws -> BroadcastTicket {
        try BroadcastTicket(address: address.trimmingCharacters(in: .whitespacesAndNewlines), fingerprint: fingerprint.trimmingCharacters(in: .whitespacesAndNewlines), invite: invite.trimmingCharacters(in: .whitespacesAndNewlines), audio: audio)
    }
    func scan() {
        discovery?.close(); devices = []
        do {
            discovery = try CoreSession { [weak self] event in
                guard let self else { return }
                if event.string("type") == "devices" {
                    let found = event.object("body")["devices"] as? [JSONObject] ?? []
                    self.devices = found.flatMap { device -> [String] in
                        let port = device["port"] as? Int ?? 8787
                        return (device["addresses"] as? [String] ?? []).map { "\($0):\(port)" }
                    }
                    self.status = self.devices.isEmpty ? "未发现接收端，可手动输入地址" : "选择地址后仍须核对接收端指纹和邀请"
                    self.discovery?.close(); self.discovery = nil
                } else if event.string("type") == "error" { self.status = event.object("body").string("code") }
            }
            discovery?.command("scan")
        } catch { status = error.localizedDescription }
    }
    func connectFile() {
        do { try sender.connect(ticket(), mirror: false) } catch { status = error.localizedDescription }
    }
    func shareFile(_ result: Result<[URL], Error>) {
        do {
            guard let url = try result.get().first else { return }
            _ = url.startAccessingSecurityScopedResource()
            do { try sender.shareFile(url, localAddress: localAddress) }
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
        do { sender.stop(); try BroadcastStore.save(ticket()); broadcastPrepared = true; status = "120 秒内点击系统广播按钮，并在接收端确认。声音只取应用音频。" }
        catch { status = error.localizedDescription }
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
        guard let source = sources.first(where: { $0.id == selectedSource }) else { status = "请先选择窗口或显示器"; return }
        let current = UUID(); generation = current
        let wantsAudio = audio
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
        do { try sender.connect(ticket(), mirror: true) } catch { status = error.localizedDescription }
    }
    #endif
    private func stopCapture() {
        generation = UUID(); connected = false
        #if os(macOS)
        Task { await capture.stop() }
        #endif
    }
    func stop() {
        stopCapture(); sender.stop(); discovery?.close(); discovery = nil; broadcastPrepared = false
        #if os(iOS)
        BroadcastStore.clear()
        #endif
    }
    func suspendHost() { stopCapture(); sender.stop(); discovery?.close(); discovery = nil }
}
#endif
