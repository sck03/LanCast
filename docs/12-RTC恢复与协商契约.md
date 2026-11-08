# 12 RTC 恢复与协商契约（D10）

状态：已实现，自动化验证与真机范围分别见08。该决策覆盖自有接收端镜像，文件和 DLNA 继续使用各自的生命周期。

## 模块职责

| 模块 | 职责 | 不持有的资源 |
|---|---|---|
| cast-core/recovery | 注入毫秒时钟、连接状态、固定截止时间、下一动作截止和重试动作 | I/O、UUID、网络、采集器 |
| cast-adapters/rtc_recovery | 将策略绑定 sessionId/negotiationId，过滤过时状态 | 平台 PeerConnection |
| WSS 连接适配 | 本地媒体状态与远端状态汇合，复用单个截止计时器发出重建或停止事件 | 屏幕像素、音频 PCM |
| Windows RtcChannel | 更新传输、SSRC、ICE/DTLS 与回调令牌，连接后请求关键帧 | WGC、硬件编码器、WASAPI 生命周期 |
| Android RtcPeer | 重建 PeerConnection，重新绑定已有轨道并应用编码参数 | 不重新使用 MediaProjection 授权 Intent |
| Apple RtcSession | 重建 PeerConnection、轨道绑定与渲染连接 | 不重启 ScreenCaptureKit/ReplayKit 或重新创建声音采集授权 |

屏幕和声音源仅在首次用户授权后创建。恢复复用已有源，停止、权限撤销和控制连接断开仍释放全部资源。仅重建传输，不把系统录屏授权纳入自动重试。

## 协议增量

保留协议 version=1，以 `session.start.body.rtcRecovery="replace-v1"` 请求能力；接收端在 `session.accepted` 和本地 `session.started` 中回显。未协商能力的旧客户端维持断流终止行为。

| 消息 | 方向与字段 | 约束 |
|---|---|---|
| rtc.offer | 发送→接收；sdp、negotiationId，重建时加 previousNegotiationId | 首次无父 ID；以后必须指向当前 ID；新 ID 不得重复 |
| rtc.answer / rtc.ice | 原有方向与字段 | 只投递给当前会话和协商；重建前的 SDP/ICE 被丢弃 |
| rtc.state | 媒体→本地控制；接收端经 WSS 转发给发送端 | negotiationId；state 仅 connected/disconnected/failed；发送端自己的报告不发往网络 |
| rtc.restart | 发送端控制→本地媒体，永不发往接收端 | 当前 negotiationId、attempt；媒体完成新 offer 后恢复投递 |
| session.state ready | 接收→发送 | 启用恢复时必须带当前 negotiationId；重复 ready 幂等 |
| session.stop | 双向 | 恢复耗尽 reason=RTC_RECOVERY_EXHAUSTED |

offer 保留请求 ID 幂等缓存；错误的父代次不改变当前状态。接收端旧媒体回调产生的本地 answer/ICE/state/ready 不得使新会话失败。连接关闭不发起自动配对，也不复用已消费邀请。

## 时间与失败行为

首次连接等待两端报告 connected，最多20秒。任一端报告 disconnected/failed 后开始一次15秒恢复窗口，在窗口开始后第1、5、10秒最多触发三次传输重建。重复失败不会延长窗口；只有当前传输两端均 connected 才结束该窗口。每次重建重置两端状态，单边连接不算成功。

临时断流在第1秒之前自行恢复时不重建。超时、用户停止、录屏撤销、SDP/编码初始化错误均终止；不会无限捕获或无限重播。控制心跳有独立截止时间，持续发送本地消息不会延长接收超时，WSS 写入也有5秒上限。

这是使用新 PeerConnection 和新 ICE/DTLS 状态完成的恢复，不依赖不同平台库对原地 ICE restart API 的一致支持。完整 IP/网卡切换导致 WSS 断开时，仍需重新配对与授权分享。

## 验证

纯策略测试覆盖重复失败、单边恢复、耗尽、时钟回退和恢复后再断流。D22补充首次截止前失败、截止后成功先于轮询到达的回归；状态报告本身必须检查截止，不能等下一次tick才判断。真实 Rust/WSS 集成覆盖能力协商、新旧 offer、过期 answer/ICE 过滤与停止。Windows 用两次 ICE/DTLS/SRTP 通路验证 H.264/Opus RTP，Mac 在重建后实际解码 H.264 帧。各轮实际构建见08；MediaProjection 真机、跨平台断流、旋转、声音恢复和完整网络切换仍须按06验收。

## D36 协商重放与按截止调度

0.6.21 / 29，2026-11-08。旧实现会在每次本机offer到达时清除`awaiting_offer`。当控制核心已经要求重建传输，重复的旧offer会重新放行旧传输的connected报告；两端旧报告到齐后，恢复策略可能误判连接已恢复并撤销15秒窗口。另一个缺口是本机offer未校验父协商，新ID但父代次错误的offer也能覆盖发送端当前协商，而接收端会拒绝它。两项回归先在原实现失败，修复后通过。

`rtc_recovery`现在仅在sessionId匹配、新协商UUID有效且不同于当前ID、previousNegotiationId准确指向当前协商时更新状态。首次offer要求父字段缺省；显式null、无效UUID及错误父代次不更新状态。重复offer仍可沿既有网络请求幂等路径处理，但不能解除等待新offer的状态。状态报告复用同一`accepts`判断，删除重复的会话/代次检查。

`cast-core/recovery::next_deadline`只返回注入时钟下的毫秒数，不引入Tokio或其他运行依赖。首次连接截止、三个重试时刻与窗口结束均由纯策略决定；两端连接成功后无需恢复计时，下一次失败再设置。`control/client`使用同一Tokio单调时钟，复用一个Sleep，在恢复动作截止和最近接收消息后15秒心跳截止之间选择较早者。每次处理消息后也检查截止；本机发送不能续期，截止边界到达的远端消息不能复活已过期连接。阻塞写入仍受既有5秒上限与关闭取消约束。

移除250毫秒interval后，空闲或已稳定连接不再为恢复检查每秒唤醒四次，也没有错过tick后的补偿轮询。重试不再被固定轮询向后取整。此处描述调度路径变化，不是CPU、RSS或投屏延迟的实测百分比；既有服务端5秒心跳及实际网络流量仍会唤醒连接。

新增纯策略回归覆盖下一截止、健康连接取消计时、重复故障、初始截止优先和饱和时钟；使用暂停时钟的真实WebSocket内存回环覆盖偏离250毫秒边界的失败，在第1、5、10秒重试、第15秒停止，以及重复旧offer和旧成功报告不能取消恢复。另验证远端消息续期、本机发送不续期。平台构建、清理和实机边界见[08](08-实施进度与审阅入口.md)。
