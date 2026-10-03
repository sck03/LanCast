import SwiftUI
import AVKit
import UniformTypeIdentifiers
#if os(iOS)
import ReplayKit
#endif

@main
struct LanCastApp: App {
    var body: some Scene { WindowGroup { ContentView() } }
}

struct ContentView: View {
    @StateObject private var receiver = ReceiverModel()
    #if !os(tvOS)
    @StateObject private var sender = SenderModel()
    @State private var mode = 0
    @State private var importing = false
    #endif
    @Environment(\.scenePhase) private var scenePhase
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("LanCast").font(.largeTitle.bold())
            #if !os(tvOS)
            Picker("用途", selection: $mode) { Text("接收投屏").tag(0); Text("发送投屏").tag(1) }.pickerStyle(.segmented)
                .onChange(of: mode) { _ in receiver.stop(); sender.stop() }
            if mode == 0 { receiverPanel } else { senderPanel }
            #else
            receiverPanel
            #endif
        }.padding(24)
        .onChange(of: scenePhase) { phase in
            if phase == .background {
                receiver.stop()
                #if !os(tvOS)
                // iOS broadcasting runs independently in its extension; host file sharing ends.
                sender.suspendHost()
                #endif
            }
        }
        .onDisappear {
            receiver.stop()
            #if !os(tvOS)
            sender.stop()
            #endif
        }
        .alert(item: $receiver.approval) { request in
            Alert(title: Text("允许此设备投屏？"), message: Text("\(request.name)\n\(request.address)"),
                  primaryButton: .default(Text("允许"), action: { receiver.approve(true, connection: request.id) }),
                  secondaryButton: .cancel(Text("拒绝"), action: { receiver.approve(false, connection: request.id) }))
        }
    }
    private var receiverPanel: some View {
        VStack(alignment: .leading, spacing: 10) {
            TextField("本机 IPv4:8787", text: $receiver.address).disabled(receiver.listening)
            HStack {
                Button("启动接收", action: receiver.start).disabled(receiver.listening)
                Button("更新邀请", action: receiver.refreshInvite).disabled(!receiver.listening)
                Button("停止投屏", action: receiver.stopMedia)
            }
            Text(receiver.status)
            if !receiver.invite.isEmpty {
                Text("SHA-256：\(receiver.fingerprint)\n邀请：\(receiver.invite)\n邀请单次有效，120 秒过期").font(.caption)
                    #if !os(tvOS)
                    .textSelection(.enabled)
                    #endif
            }
            ZStack {
                Color.black
                if let player = receiver.player { VideoPlayer(player: player) }
                else if let video = receiver.video { VideoSurface(track: video) }
                else { Text("等待画面").foregroundStyle(.white) }
            }.frame(minHeight: 220)
        }
    }
    #if !os(tvOS)
    private var senderPanel: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 12) {
                HStack { TextField("接收端 IPv4:8787", text: $sender.address); Button("扫描", action: sender.scan) }
                ForEach(sender.devices, id: \.self) { address in Button(address) { sender.address = address } }
                TextField("核对接收端完整 SHA-256 指纹", text: $sender.fingerprint)
                SecureField("一次性邀请", text: $sender.invite)
                Toggle("分享系统／应用声音（不使用麦克风）", isOn: $sender.audio)
                #if os(macOS)
                HStack {
                    Button("选择屏幕或窗口", action: sender.refreshSources)
                    Picker("来源", selection: $sender.selectedSource) { ForEach(sender.sources) { Text($0.name).tag($0.id) } }
                }
                Text("窗口画面可能伴随系统全局声音；受保护内容由系统限制。").font(.caption)
                Button("开始屏幕分享", action: sender.startMirror)
                #elseif os(iOS)
                Button("准备屏幕广播", action: sender.prepareBroadcast)
                if sender.broadcastPrepared { BroadcastPicker().frame(width: 60, height: 60) }
                Text("系统广播由扩展负责；停止请使用系统录屏指示器。准备配置过期后请更新邀请。").font(.caption)
                #endif
                Divider()
                Text("MP4 原文件播放").font(.headline)
                TextField("本机局域网 IPv4", text: $sender.localAddress)
                HStack {
                    Button("连接以发送文件", action: sender.connectFile)
                    Button("选择 MP4") { importing = true }.disabled(!sender.connected)
                }
                HStack { Button("播放") { sender.playback("play") }; Button("暂停") { sender.playback("pause") } }
                HStack { TextField("跳转秒数", value: $sender.position, format: .number); Button("跳转") { sender.playback("seek") } }
                Slider(value: $sender.volume, in: 0...1) { Text("播放音量") }.onChange(of: sender.volume) { _ in sender.playback("volume") }
                Button("停止当前分享", action: sender.stop)
                Text(sender.status)
            }
        }.fileImporter(isPresented: $importing, allowedContentTypes: [.mpeg4Movie], allowsMultipleSelection: false, onCompletion: sender.shareFile)
    }
    #endif
}

#if os(iOS)
struct BroadcastPicker: UIViewRepresentable {
    func makeUIView(context: Context) -> RPSystemBroadcastPickerView {
        let view = RPSystemBroadcastPickerView(frame: .zero)
        view.preferredExtension = (Bundle.main.bundleIdentifier ?? "dev.lancast.ios") + ".broadcast"
        view.showsMicrophoneButton = false; return view
    }
    func updateUIView(_ uiView: RPSystemBroadcastPickerView, context: Context) {}
}
#endif
