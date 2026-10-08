import SwiftUI
import AVKit
import UniformTypeIdentifiers
import LanCastContracts
#if os(iOS)
import ReplayKit
#endif

@main
struct LanCastApp: App {
    var body: some Scene { WindowGroup { ContentView() } }
}

struct ContentView: View {
    @StateObject private var receiver = ReceiverModel()
    @State private var receiverAdvanced = false
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
            #if !os(macOS)
            if phase == .background {
                receiver.stop()
                #if !os(tvOS)
                // iOS broadcasting runs independently in its extension; host file sharing ends.
                sender.suspendHost()
                #endif
            }
            #endif
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
            Text(receiver.address.isEmpty ? "未连接局域网" : "本机网络 · \(receiver.address)").font(.subheadline).foregroundStyle(.secondary)
            Button(receiverAdvanced ? "收起网络设置" : "网络设置") { receiverAdvanced.toggle() }
            if receiverAdvanced {
                Picker("本机网络", selection: $receiver.networkChoice) {
                    Text("自动选择").tag("")
                    ForEach(receiver.availableNetworks, id: \.self) { Text($0).tag($0) }
                }.disabled(receiver.listening).onChange(of: receiver.networkChoice) { _ in receiver.refreshNetwork() }
                Button("重新识别网络", action: receiver.refreshNetwork).disabled(receiver.listening)
                #if !os(tvOS)
                Toggle("自定义监听地址", isOn: $receiver.manualAddress).disabled(receiver.listening).onChange(of: receiver.manualAddress) { _ in receiver.refreshNetwork() }
                if receiver.manualAddress { TextField("本机 IPv4:8787", text: $receiver.address).disabled(receiver.listening) }
                #endif
            }
            HStack {
                Button("启动接收", action: receiver.start).disabled(receiver.listening)
                Button("刷新配对码", action: receiver.refreshInvite).disabled(!receiver.listening)
                Button("停止投屏", action: receiver.stopMedia)
            }
            Text(receiver.status)
            if !receiver.invite.isEmpty {
                Text("配对码  \(receiver.invite)").font(.title.monospacedDigit().bold())
                Text("2 分钟内单次有效。请核对发送端显示的完整指纹：\n\(ConnectionHints.displayFingerprint(receiver.fingerprint))").font(.callout.monospaced())
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
        }.onAppear(perform: receiver.refreshNetwork)
    }
    #if !os(tvOS)
    private var senderPanel: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 12) {
                Text("1  选择接收设备").font(.headline)
                HStack { Text(sender.localAddress.isEmpty ? "未连接局域网" : "本机网络 · \(sender.localAddress)").foregroundStyle(.secondary); Spacer(); Button(sender.scanning ? "搜索中…" : "刷新设备", action: sender.scan).disabled(sender.connectionLocked) }
                ForEach(sender.devices) { device in
                    Button { sender.selectDevice(device) } label: {
                        HStack { Image(systemName: sender.selectedDevice == device.id ? "checkmark.circle.fill" : "tv"); Text(device.name); Spacer(); Text(device.address).font(.caption).foregroundStyle(.secondary) }
                    }.disabled(sender.connectionLocked)
                }
                TextField("电视上的 8 位配对码", text: $sender.invite).autocorrectionDisabled().disabled(sender.connectionLocked)
                    #if os(iOS)
                    .keyboardType(.numberPad)
                    #endif
                DisclosureGroup("网络设置", isExpanded: $sender.advanced) {
                    Picker("本机网络", selection: $sender.networkChoice) {
                        Text("自动选择").tag("")
                        ForEach(sender.availableNetworks, id: \.self) { Text($0).tag($0) }
                    }.onChange(of: sender.networkChoice) { _ in sender.refreshNetwork() }
                    Button("重新识别网络", action: sender.refreshNetwork)
                }.disabled(sender.connectionLocked)
                Text("选中设备会填入地址和指纹；开始连接时，请核对完整指纹与电视一致。").font(.caption).foregroundStyle(.secondary)
                Divider()
                Text("2  选择分享内容").font(.headline)
                Toggle("分享系统／应用声音（不使用麦克风）", isOn: $sender.audio).disabled(sender.connectionLocked)
                #if os(macOS)
                HStack {
                    Button("选择屏幕或窗口", action: sender.refreshSources)
                    Picker("来源", selection: $sender.selectedSource) { ForEach(sender.sources) { Text($0.name).tag($0.id) } }
                }.disabled(sender.connectionLocked)
                Text("窗口画面可能伴随系统全局声音；受保护内容由系统限制。").font(.caption)
                Button("开始屏幕分享", action: sender.startMirror).disabled(sender.connectionLocked)
                #elseif os(iOS)
                Button("准备屏幕广播", action: sender.prepareBroadcast).disabled(sender.connectionLocked)
                if sender.broadcastPrepared { BroadcastPicker().frame(width: 60, height: 60) }
                Text("系统广播由扩展负责；停止请使用系统录屏指示器。准备配置过期后请更新邀请。").font(.caption)
                #endif
                Divider()
                Text("MP4 原文件播放").font(.headline)
                HStack {
                    Button("连接以发送文件", action: sender.connectFile).disabled(sender.connectionLocked)
                    Button("选择 MP4") { importing = true }.disabled(!sender.connected || !sender.fileConnection || sender.fileShared)
                }
                HStack { Button("播放") { sender.playback("play") }; Button("暂停") { sender.playback("pause") } }.disabled(!sender.connected || !sender.fileConnection || !sender.fileShared)
                HStack { TextField("跳转秒数", value: $sender.position, format: .number); Button("跳转") { sender.playback("seek") } }.disabled(!sender.connected || !sender.fileConnection || !sender.fileShared)
                Slider(value: $sender.volume, in: 0...1) { Text("播放音量") }.onChange(of: sender.volume) { _ in sender.playback("volume") }.disabled(!sender.connected || !sender.fileConnection || !sender.fileShared)
                Button("停止当前分享", action: sender.stop)
                Text(sender.status)
            }
        }.onAppear { sender.refreshNetwork(); if sender.devices.isEmpty { sender.scan() } }
        .fileImporter(isPresented: $importing, allowedContentTypes: [.mpeg4Movie], allowsMultipleSelection: false, onCompletion: sender.shareFile)
        .alert(item: $sender.confirmation) { request in
            Alert(title: Text("核对接收端身份"), message: Text("确认以下完整指纹与接收端一致：\n\n\(ConnectionHints.displayFingerprint(request.ticket.fingerprint))\n\n\(request.ticket.address)"),
                  primaryButton: .default(Text("一致，继续"), action: { sender.confirm(request) }),
                  secondaryButton: .cancel(Text("取消"), action: sender.cancelConfirmation))
        }
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
