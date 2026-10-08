# 17 Android 与 Apple 连接体验

决策 D14；2026-10-08；续接 D13 的 Windows 和共享配对协议，应用版本 0.4.1 / 5。

## 当前流程

- Android 发送：进入页面自动识别本机网络并扫描 LanCast/DLNA；统一列表显示名称、类型和地址。选择新版 LanCast 后填入地址与指纹，用户只输入电视配对码并核对完整指纹，然后在电视允许连接。
- Android 接收：默认自动选择本机网络，点击“启动接收”；大字显示 8 位码和分组完整指纹。高级网络设置仅用于特殊网卡/地址，原来的输入框不再占据主流程。
- macOS、iOS/iPadOS 发送：进入发送页自动扫描，用设备名称选择，填入完整身份提示。原地址/指纹/本机地址输入收进高级设置；连接或广播准备前弹出完整指纹核对。
- Mac/iOS/tvOS 接收：默认识别本机网络；网络选择收进设置，配对码及身份信息直接显示。启动接收仍由用户操作，接收端仍须明确批准新连接。
- Mac 最小化或进入后台不主动停止会话，可从 Dock 恢复；关闭窗口仍停止。Android 实际采集继续由前台服务负责，iOS 后台屏幕分享仍由 ReplayKit 广播扩展负责，宿主普通文件会话不承诺后台保活。

扫描不发起连接或采集。旧接收端没有 `fingerprint` 提示时保留手动填写入口，旧长邀请也可输入；显示简化不降低完整 SPKI 绑定或系统录屏授权要求。Apple 当前没有 DLNA 适配，本轮没有添加或宣传 Apple DLNA 发送。

## 模块边界

| 模块 | 职责 |
|---|---|
| Android `control-bridge/ConnectionHints.kt` | 地址/指纹校验、公开 ReceiverHint、不可变 PairingInput、有界目录、一次性 CaptureGrantGate；无控件或JNI调用 |
| Android `control-bridge/LocalNetwork.kt` | 网卡枚举、优先本机活跃 Wi-Fi/以太网、子网选择和显式网卡覆盖；无网络探测包 |
| Android `app-sender/ConnectionForm.kt` | 原生字段、折叠设置、列表和核对框；通过回调提交已核对的不可变输入，不持有核心/媒体 |
| Android `SenderRuntime` | 发现代次、两种结果合并、限时 multicast lock、核心连接及与现有媒体生命周期协调 |
| Apple `LanCastContracts/DiscoveryContracts.swift` | Foundation 值类型与验证，不持有网络/SwiftUI/媒体对象 |
| Apple `Shared/Control/LocalAddresses.swift` | 以太网/Wi-Fi/显式桥接地址枚举，按系统路由选择来源；UDP connect 只查路由，不发送数据或查询DNS |
| Apple `SenderModel` | 设备选择、不可变确认快照、UI状态；确认时验证请求ID和票据有效期，再交给 SenderSession/广播存储 |
| Apple `SenderSession` / `ReceiverModel` | 原有控制/媒体会话；暴露连接等待状态，接收模型管理自动网络与用户明确启动 |

Android 发送/接收复用同一 drawable 图标，资源移入 control-bridge，避免两份标识源文件；发送端补齐启动图标。没有引入新的运行框架。JUnit 与 JVM JSON 实现仅用于测试，不进入 APK。

## 状态与安全

切换设备清空旧指纹和配对码。Android 两类扫描使用共享核心的 `scanGeneration`，超时或重建后丢弃旧结果；multicast lock 在完成/超时/关闭时释放。Apple 为每次扫描持有独立 CoreSession，关闭旧会话使旧回调失效。

配对和媒体准备期间锁定选择字段。Android 系统授权/文件选择期间保持当前目标；文件开始前重新选择有效本机地址。播放暂停/跳转仅对当前文件开放。停止仍撤销媒体和资源，不绕过旧的用户授权策略。

Android 从系统授权返回到前台服务时，使用一次性请求ID绑定接收地址、DLNA设备与本机网络。停止/取消使请求失效，改变目标或重复使用不能启动采集；旧服务销毁只停止自己持有的采集会话。这个门控只验证应用自己的请求，不生成或替代系统录屏授权。

Apple 核对框绑定不可变 BroadcastTicket 和操作类型；确认ID单独保留，兼容 SwiftUI 先清除 alert 绑定再执行按钮动作。修改身份字段会使未确认请求失效，并清除旧的广播准备配置。文件、镜像、iOS广播不能悄悄共用错误的目标/声音选项。

自动网络识别仅支持本项目的 LAN IPv4 范围；Android 根据本机网络元数据和子网优先级选择，Apple 使用系统路由。复杂VPN/多网卡仍提供手动选择，不能保证任意自定义路由策略均自动正确。

## 验证入口与边界

Android `ConnectionHintsTest` 的3项测试覆盖私有IPv4、非规范/错误地址、子网、指纹规范化、同ID跨协议、去重、旧接收端缺失指纹，以及采集请求停止/换目标/重复返回。`build-android.py` 在组装/lint三个产品之外执行 control-bridge 对应构建模式的 JVM 单元测试。

Apple `DiscoveryContractsTests` 覆盖地址过滤、畸形记录、IPv6优先列表、名称归一、重复设备和旧身份清空；由 macOS 构建入口的 Swift Package 契约测试执行，原有WSS/RTC集成测试继续保留。

每个平台的实际构建提交、结果与产物核对范围记在[08](08-实施进度与审阅入口.md)。没有可用真机时，不将源码/模拟器编译写成电视操作、系统权限、后台长稳或真实网络切换验收。
