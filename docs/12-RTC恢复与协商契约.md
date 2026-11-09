# 12 RTC 恢复与协商契约（D10）

状态：已实现，自动化验证与真机范围分别见08。该决策覆盖自有接收端镜像，文件和 DLNA 继续使用各自的生命周期。

## 模块职责

| 模块 | 职责 | 不持有的资源 |
|---|---|---|
| cast-core/recovery | 注入毫秒时钟、连接状态、固定截止时间、下一动作截止和重试动作 | I/O、UUID、网络、采集器 |
| cast-adapters/rtc_recovery | 将策略绑定 sessionId/negotiationId，判断offer是否允许发送并过滤过时状态 | 平台 PeerConnection |
| WSS 连接适配 | 本地媒体状态与远端状态汇合，复用单个截止计时器发出重建或停止事件 | 屏幕像素、音频 PCM |
| Windows RtcChannel | 更新传输、SSRC、ICE/DTLS 与回调令牌，连接后请求关键帧 | WGC、硬件编码器、WASAPI 生命周期 |
| Android RtcPeer | 重建 PeerConnection，重新绑定已有轨道并应用编码参数 | 不重新使用 MediaProjection 授权 Intent |
| Apple RtcSession | 重建 PeerConnection、轨道绑定与渲染连接 | 不重启 ScreenCaptureKit/ReplayKit 或重新创建声音采集授权 |

屏幕和声音源仅在首次用户授权后创建。恢复复用已有源，停止、权限撤销和控制连接断开仍释放全部资源。仅重建传输，不把系统录屏授权纳入自动重试。

## 协议增量

保留协议 version=1，以 `session.start.body.rtcRecovery="replace-v1"` 请求能力；接收端在 `session.accepted` 和本地 `session.started` 中回显。未协商恢复能力的会话在断流时终止；这属于会话能力判断，不提供旧配对协议回退。

| 消息 | 方向与字段 | 约束 |
|---|---|---|
| rtc.offer | 发送→接收；sdp、negotiationId，重建时加 previousNegotiationId | 首次无父 ID；以后必须指向当前 ID；新 ID 不得重复 |
| rtc.answer / rtc.ice | 原有方向与字段 | 只投递给当前会话和协商；重建前的 SDP/ICE 被丢弃 |
| rtc.state | 媒体→本地控制；接收端经 WSS 转发给发送端 | negotiationId；state 仅 connected/disconnected/failed；发送端自己的报告不发往网络 |
| rtc.restart | 发送端控制→本地媒体，永不发往接收端 | 当前 negotiationId、attempt；媒体完成新 offer 后恢复投递 |
| session.state ready | 接收→发送 | 镜像必须已有当前 negotiationId；发送端恢复等待新offer时丢弃旧ready；文件播放不需要协商ID，重复ready幂等 |
| session.stop | 双向 | 恢复耗尽 reason=RTC_RECOVERY_EXHAUSTED |

发送端在写入网络前丢弃重复、过期及父代次不匹配的offer；接收端保留请求 ID 幂等缓存和独立校验。错误的父代次不改变当前状态。接收端旧媒体回调产生的本地 answer/ICE/state/ready 不得使新会话失败。连接关闭不发起自动配对，也不复用已消费邀请。

D40将发送端当前会话及启动请求关联移入`control/client_session`。旧会话的stop/error不能取消当前恢复，重复确认不能重新创建恢复计时；本机过期会话信令在网络发送前过滤。具体规则和文件会话共同维护于[14](14-控制连接生命周期与维护边界.md#d40-发送会话关联与过期消息隔离)。

## 时间与失败行为

首次连接等待两端报告 connected，最多20秒。任一端报告 disconnected/failed 后开始一次15秒恢复窗口，在窗口开始后第1、5、10秒最多触发三次传输重建。重复失败不会延长窗口；只有当前传输两端均 connected 才结束该窗口。每次重建重置两端状态，单边连接不算成功。

临时断流在第1秒之前自行恢复时不重建。超时、用户停止、录屏撤销、SDP/编码初始化错误均终止；不会无限捕获或无限重播。控制心跳有独立截止时间，持续发送本地消息不会延长接收超时，WSS 写入也有5秒上限。

这是使用新 PeerConnection 和新 ICE/DTLS 状态完成的恢复，不依赖不同平台库对原地 ICE restart API 的一致支持。完整 IP/网卡切换导致 WSS 断开时，仍需重新配对与授权分享。

## 验证

纯策略测试覆盖重复失败、单边恢复、耗尽、时钟回退和恢复后再断流。D22补充首次截止前失败、截止后成功先于轮询到达的回归；状态报告本身必须检查截止，不能等下一次tick才判断。真实 Rust/WSS 集成覆盖能力协商、新旧 offer、过期 answer/ICE 过滤与停止。Windows 用两次 ICE/DTLS/SRTP 通路验证 H.264/Opus RTP，Mac 在重建后实际解码 H.264 帧。各轮实际构建见08；MediaProjection 真机、跨平台断流、旋转、声音恢复和完整网络切换仍须按06验收。

## D36 协商重放与按截止调度

0.6.21 / 29，2026-11-08。旧实现会在每次本机offer到达时清除`awaiting_offer`。当控制核心已经要求重建传输，重复的旧offer会重新放行旧传输的connected报告；两端旧报告到齐后，恢复策略可能误判连接已恢复并撤销15秒窗口。另一个缺口是本机offer未校验父协商，新ID但父代次错误的offer也能覆盖发送端当前协商，而接收端会拒绝它。两项回归先在原实现失败，修复后通过。

`rtc_recovery`仅在sessionId匹配、新协商UUID有效且不同于当前ID、previousNegotiationId准确指向当前协商时更新状态。首次offer要求父字段缺省；显式null、无效UUID及错误父代次不更新状态。D36只阻止这些offer改变恢复状态；D41进一步阻止其进入网络，取代此前继续发送并依赖远端请求缓存的行为。状态报告复用同一`accepts`判断，删除重复的会话/代次检查。

`cast-core/recovery::next_deadline`只返回注入时钟下的毫秒数，不引入Tokio或其他运行依赖。首次连接截止、三个重试时刻与窗口结束均由纯策略决定；两端连接成功后无需恢复计时，下一次失败再设置。`control/client`使用同一Tokio单调时钟，复用一个Sleep，在恢复动作截止和最近接收消息后15秒心跳截止之间选择较早者。每次处理消息后也检查截止；本机发送不能续期，截止边界到达的远端消息不能复活已过期连接。阻塞写入仍受既有5秒上限与关闭取消约束。

移除250毫秒interval后，空闲或已稳定连接不再为恢复检查每秒唤醒四次，也没有错过tick后的补偿轮询。重试不再被固定轮询向后取整。此处描述调度路径变化，不是CPU、RSS或投屏延迟的实测百分比；既有服务端5秒心跳及实际网络流量仍会唤醒连接。

新增纯策略回归覆盖下一截止、健康连接取消计时、重复故障、初始截止优先和饱和时钟；使用暂停时钟的真实WebSocket内存回环覆盖偏离250毫秒边界的失败，在第1、5、10秒重试、第15秒停止，以及重复旧offer和旧成功报告不能取消恢复。另验证远端消息续期、本机发送不续期。平台构建、清理和实机边界见[08](08-实施进度与审阅入口.md)。

## D41 发送前过滤过期offer

0.6.26 / 34，2026-11-10。此前恢复模块拒绝重复或错误父代次的offer时仅保持本地状态，`client_session`仍将消息交给网络。迟到媒体回调若生成新的请求ID，接收端幂等缓存不能命中，便返回`STALE_NEGOTIATION`；发送端将该错误交付宿主并清除当前会话，导致已经恢复的投屏再次终止。内存WebSocket回环和真实TLS/WSS回环均在修复前复现。

`RtcRecovery::offer`返回是否接受本次协商，`client_session`仅放行接受的offer。状态更新与发送判定使用同一套会话、UUID和父代次规则；网络循环无需增加协商判断。重复当前offer、旧offer、新ID但缺少/错误父代次，以及显式null父字段均在WSS序列化前丢弃。首个合法offer及正确继承当前代次的替换offer仍正常传送，当前answer/ICE/ready继续交付。未协商恢复的会话仍遵循原有能力规则。

回归还验证等待重建时重复offer不能重新开放旧connected报告，第1、5、10秒重试和第15秒终止保持有效。新增返回值不增加状态、历史缓存、依赖、线程或计时器；减少无效SDP序列化、网络写入、接收端解析及错误回传，未量化CPU/RSS或端到端延迟。实际测试、构建与清理记录见08。

## D43 播放就绪与协商生命周期

0.6.28 / 36，2026-11-10。接收端此前用两个可选UUID直接比较：首个offer到达前，缺失/无效的ready协商ID与当前空协商相等，镜像会话可能从Negotiating提前进入Streaming。未启用恢复能力时ready还会绕过协商检查。ICE、answer和rtc.state的无效ID已由协议解析器拒绝，不属于本次确认的入口漏洞。

`session::Session::accepts_negotiation`统一核对镜像模式、未停止状态、会话ID和已存在的协商ID；接收端answer/ICE/state、镜像ready及远端ICE复用此判断。`Session::ready`同时保护状态机不允许尚未建立协商的镜像进入Streaming。ready缺失ID、null、无效UUID或旧ID在接收端排队发送前丢弃，不因恢复能力开关改变规则；文件会话仍由Loading进入Playing，不要求RTC字段。

发送端的`control/client_session`将ready纳入既有`RtcRecovery::accepts`过滤。收到重建请求后到合法替换offer之前，旧ready不可交付宿主；新offer提交后，迟到的上一代ready也被丢弃。这覆盖旧ready在接收端通过校验后已排入网络、随后发送端切换代次的情况。当前ready继续交付，重复ready幂等；ready不能代替双端connected报告取消恢复截止。

两项新增回归先在旧实现失败，修复后通过；扩展现有WebSocket内存回环、真实TLS/WSS回环、文件会话及1/5/10秒重试、15秒截止测试。生产路径删除重复的UUID判断，不新增依赖、线程、计时器、状态缓存或旧应用兼容分支；无效ready在队列/宿主JSON编码之前过滤，减少无效网络发送和平台回调。CPU/RSS、延迟和真实设备长稳仍未测量；验证与清理证据见08。
