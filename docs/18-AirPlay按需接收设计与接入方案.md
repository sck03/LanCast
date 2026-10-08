# 18 AirPlay按需接收设计与接入方案

决策：D15；日期：2026-10-08。目标：AirPlay 2优先，向下兼容AirPlay 1/legacy镜像。0.6.0已选择修正后的固定`rairplay`源码接入独立Android Airplay变体，具体模块、配对/加密媒体回环与GPL分发见[20实施记录](20-AirPlay实现与审阅指南.md)，构建结果见[08](08-实施进度与审阅入口.md)。本章继续定义产品合同，19保留前期选型证据；现代/旧模式的真实苹果和电视互通仍待验收。

## 1. 用户需求与范围

iPhone/iPad无法安装LanCast发送App时，用户可先打开电视上的LanCast接收App，手动开启“苹果原生投屏（AirPlay）”，再从苹果设备控制中心的“屏幕镜像”选择电视。手机使用系统发送能力；电视必须能够安装并运行LanCast接收端。

这是Android电视/盒子接收端的可选能力。开关默认关闭，需要时才开启；不做开机自启动，不要求厂商预装、系统签名或长期保活白名单。用户启用期间，接收服务可与页面分离，在系统允许的条件下继续后台监听。后台显示行为见第3节，不能把后台监听等同于自动弹出画面。

首阶段目标是iPhone/iPad系统屏幕镜像及镜像伴随声音，验证现代AirPlay 2优先和旧模式兼容。UxPlay的Legacy Mirror是可参考的兼容实现，不能以它取代现代AP2的验收，具体选择与重连策略见第6节。能力按协议协商、苹果系统版本和电视实际解码结果声明；“支持AirPlay”不代表已支持所有AirPlay 2功能、独立音频、多房间音频、应用内视频直放、DRM内容或任意未来系统版本。macOS、Windows、iOS及tvOS版LanCast不随本决策增加AirPlay接收功能。

AirPlay开关可以独立开启，不要求先启动自有WSS接收。与LanCast WSS/WebRTC或文件通道同时启用时，共用单路媒体占用。电视自身已有的AirPlay服务属于系统能力；打开或关闭LanCast开关只管理本应用的服务。

## 2. 开关与用户操作合同

用户界面使用“苹果原生投屏（AirPlay）”，不暴露mDNS、RTSP或协议库名称。启用后显示接收名称、运行状态和关闭入口，提示手机从控制中心选择“屏幕镜像”。

| 操作/条件 | 预期行为 |
|---|---|
| 首次启动App，或接收服务已结束后重新启动 | AirPlay关闭；不因上一次使用过而自动开启 |
| 回到仍在运行的接收服务页面 | 从服务读取真实状态，继续显示已开启；页面重建不重复启动引擎 |
| 在可见页面打开开关 | 检查当前构建、网络、平台服务条件与引擎；启动成功并完成服务发布后显示“等待苹果设备连接” |
| 启动失败或构建未包含引擎 | 显示明确原因并回到关闭状态；不发布一个无法接收媒体的AirPlay设备 |
| 按Home返回电视桌面 | 已由用户启用的服务继续监听；页面离开不关闭服务 |
| 手机停止镜像，或在播放页停止本次投屏 | 结束媒体会话，释放解码/播放资源，保持服务开启并等待下一次连接 |
| 播放页按返回 | 停止本次投屏并回到接收页；不隐式关闭已启用的AirPlay服务 |
| 接收首页按返回退出，或选择“关闭接收并退出” | 停止本应用接收服务并退出；需要后台等待时使用Home，不依赖拦截退出操作 |
| 关闭AirPlay开关，或使用服务通知的关闭操作 | 结束AirPlay会话、撤销服务发现、关闭监听并释放资源；其他独立启用的LanCast服务按自身生命周期处理 |
| 系统结束服务/进程，用户强行停止App，或电视重启 | 本次启用结束；不通过持久化开关、任务调度或自动重投递启动命令恢复，需要用户再次打开App启用 |

接收名称等非运行配置可以保存；“正在开启”状态不持久化为下次启动的指令。启用期间单次手机断开不会关闭开关，关闭开关则必须立即取消未完成连接和后续接入。

## 3. Android服务与显示生命周期

接收入口从可见Activity中的用户操作启动。下文`ReceiverService`职责在0.6.0对应`AirPlayService`：持有引擎、网络监听、必要的网络锁和状态；`AirPlayPanel`只绑定状态、提交用户操作和提供显示区域。其余设计名称与实际文件的映射由20维护。

- 在适用系统上使用用户可感知的前台服务，提供“正在等待苹果投屏/正在投屏”的运行提示，以及返回接收页和关闭的操作。前台服务不要求Activity一直可见。
- 根据Android版本、targetSdk与实际用途声明服务类型及权限。网络设备交互可评估`connectedDevice`，真实播放阶段按用途评估`mediaPlayback`；待机不可伪装成持续播放。选定类型、启动条件和停止行为必须通过目标版本验证。
- 接收端不采集本机屏幕或麦克风，不为AirPlay接收申请MediaProjection或录音权限。只申请实际需要的网络、服务与用户提示权限。
- 采用`START_NOT_STICKY`，不注册开机启动接收器，不重投递旧启动命令恢复已结束的启用周期。服务被系统重新创建而缺少有效的当次启用上下文时立即停止。
- 组播接收锁只在需要发现时持有；CPU/Wi-Fi等锁按实际阶段申请与释放。锁不能作为进程永久存活的保证，待机不初始化解码器、占用音频焦点或保持电视屏幕常亮。

后台发现、后台维持连接、后台打开播放页分别检测与记录：

1. 接收页可见时，按正常配对与会话流程使用显示区域。
2. 页面不可见时，服务仍可响应发现和受控的连接协商；仅在系统允许的条件下打开接收页。
3. 无法打开页面时，通过系统允许的通知/入口提示用户回到接收App；采用有限等待，不把尚无显示区域的连接标为“正在投屏”。苹果发送端若不能容忍等待，则结束这次协商，用户打开接收页后重新连接。
4. 配对确认或播放准备超时，释放本次连接及占用，保持已启用服务等待下一次连接。握手不得无限挂起，也不能为了维持连接无界缓存视频。
5. 播放中离开页面导致Surface不可用时，结束本次媒体会话并保留接收服务；重新打开页面不自动复播手机屏幕。

不把悬浮窗、无障碍或厂商特权设为基础功能的前提。没有后台弹出条件的电视，仍可在用户打开接收页后完成投屏；发布支持矩阵必须明确这种差别。具体握手等待预算由所选引擎和苹果客户端实测确定，在编码前固定为有上限的合同。

## 4. 模块边界与媒体接入

```text
接收页：开关、接收名称、配对提示、运行状态
    ↓ 本机命令 / 状态订阅
Android ReceiverService：用户启用周期、平台服务、网络生命周期
    ↓
接收会话协调器：LanCast与AirPlay共用的单路接收占用
    ├─ 现有WSS/WebRTC与文件适配器
    └─ 可选AirPlay适配器：发现、配对、协议、传输、时间戳
           ↓ 有界编码数据与协商结果
       Android媒体适配：视频解码、音频解码/输出、同步
           ↓
       接收播放页的Surface与系统音频输出
```

| 边界 | 职责与约束 |
|---|---|
| UI/控制器 | `enable`、`disable`、`stopCurrentSession`及状态订阅；不解析协议、不持有网络线程或解码器 |
| ReceiverService | 唯一持有当次启用代次；处理通知、网络变化、后台状态及关闭；不实现协议解析 |
| 接收会话协调器 | 为两种入口统一分配/释放媒体占用；一个接收端一次显示一个来源，不能让两个适配器各自判断“空闲” |
| AirPlay适配器 | 封装已核查的协议引擎；上报配对、协商、连接、断开与错误；对外不泄漏库对象 |
| AirPlay发现 | 只在引擎就绪后发布真实能力；可复用成熟mDNS组件，每条AirPlay记录只由一个发布器管理 |
| Android媒体适配 | 接收协商后的编码视频/音频与时间戳，适配MediaCodec、Surface和音频输出；不反向持有Activity |

以接收App内服务、可裁剪的AirPlay适配模块和独立产品变体组织代码，不引入任意插件加载。0.6.0将跨入口占用与代次放入纯Kotlin `receiver-contracts`，由Android宿主统一持有；原有Rust领域/核心保持无媒体数据的边界，两个JNI库不各自维护一份占用。逐帧数据不通过控制JSON或Rust业务层。

AirPlay协议与原有WSS/WebRTC分开握手、认证和传输。AirPlay媒体解包后直接进入原生接收媒体路径，不为接入而再编码或转发一遍WebRTC。现有显示布局、资源管理规范可以复用，但不能假设libwebrtc已提供可直接复用的独立AirPlay解码入口。

媒体接口必须明确：视频codec/profile、参数集、编码访问单元格式、帧边界、时间戳单位；音频codec/profile、配置、采样率、声道与时间戳。H.264视频和镜像音频分别验证；AAC-LC、AAC-ELD等不同配置不能视为同一种已支持能力。无法解码时收窄发现/协商能力或明确拒绝，不伪报声音可用。

媒体队列有容量与时长上限，停止可打断等待；跨JNI/C ABI规定缓冲所有权和释放时机。音画同步使用协议时间戳与校时结果，不以包到达时间代替播放时钟；方向、分辨率与参数集改变须受控重建媒体资源。

## 5. 状态、冲突与关闭

服务启用状态和单次媒体状态分别管理：

- 服务：`Disabled → Starting → Listening → Stopping → Disabled`；启动失败经过清理回到Disabled，保留失败原因。
- 单次会话：`Idle → Connecting → WaitingForDisplay → Mirroring → Stopping → Idle`；显示已准备好时可直接进入Mirroring。
- 服务已启用而网络不可用时暂停发布，显示网络不可用；网络恢复事件只在同一次有效启用周期内触发重新检查和发布，不建立脱离该周期的重启任务。

每次启用与每次媒体连接均有代次。关闭先使旧代次失效并禁止新接入，再撤销发布、取消协商、结束媒体、关闭套接字、等待工作线程退出，最后释放网络锁/前台服务。重复关闭安全；迟到回调不能重新发布设备、更新下一会话或恢复已停止的引擎。清理未完成前不启动另一个引擎实例。

第6.2节允许的兼容尝试必须先结束失败连接，再创建新会话代次；不在已协商的连接中替换协议或密钥，不复用WSS/RTC的恢复状态机。用户关闭、退出或启用周期结束会取消尚未进行的兼容尝试。

自有镜像/文件播放占用显示区域时，AirPlay新连接应拒绝或明确返回忙；反向接入同样处理。切换来源须由用户停止旧来源后再连接，不静默抢占。适配器释放媒体占用必须核对持有者及代次。

AirPlay使用其支持的标准配对/访问控制和电视端确认流程，不要求苹果系统输入LanCast的SPKI指纹或完成自有配对协议v2。发现不构成授权；未完成身份/接入确认和显示准备，不接纳媒体入屏。配对码、密钥和原始屏幕内容不写入诊断日志。具体PIN/确认交互在引擎原型阶段固定并验证。

服务关闭时发送发现撤销并停止响应；iPhone设备列表可能短暂保留发现缓存，因此验收同时检查主动浏览、端口与实际连接结果，不能要求对方界面瞬时移除。网络切换应先撤销旧接口状态、结束旧媒体，再在新网络条件满足时发布；不能因网络恢复重新开启用户已经关闭的功能。

## 6. AirPlay 2优先、向下兼容与实现选择

### 6.1 协议优先级与开源实现分开选择

优先选择双方实际支持且已验证的现代AirPlay 2镜像/声音路径，并兼容已验证的AirPlay 1/legacy镜像；不固定为Legacy优先。扩展检索发现`rairplay`具有HomeKit/Legacy配对和音视频接口，列为现代AP2原型的优先验证对象，但尚无本项目Android互通证据，不能据此直接锁定生产依赖。具体证据与其他候选比较由[19](19-AirPlay2开源接收调研与选型.md)维护。

UxPlay继续作为C/C++镜像核心的第一顺位参考和兼容路线。Android接入比较[jqssun/android-airplay-server](https://github.com/jqssun/android-airplay-server)的NDK/JNI/服务和[CastBay](https://github.com/weenas/castbay)的API23独立接收库；二者都基于UxPlay，不是两个独立的现代AP2栈。[PhairPlay](https://github.com/mazer666/PhairPlay)补充握手、时钟与测试对照。若采用Rust协议库，也只在适配层承接协议与编码数据交付，保持Kotlin界面/服务与原生媒体边界。

现代路径必须在真实设备上完成配对、控制、画面和伴随声音，才能标为该机型已支持的AP2能力。0.6.0已引入固定rairplay与PlayFair源码并实现两种配对入口；自动化证明协议回环，不证明苹果实际选路和电视完整播放。下列UxPlay结构保留为对照与未来后端替换依据，当前未同时打包第二个协议服务。

已检查上游提交`3dbf7ceee65932154e85a2f83963d53520a799fa`。这是可追溯的调研快照，不是生产版本锁定，也不因其较新就承诺稳定性：

- 上游将`lib/`构建为`airplay`库，`uxplay`可执行程序另外链接`renderers`；`lib/raop.h`提供`audio_process`、`video_process`等回调。这为复用协议核心、替换播放适配提供了具体边界。
- Android方案以NDK构建协议库和必要胶水，通过窄C ABI/JNI接入ReceiverService，使用Android媒体适配承接编码数据、时间戳、配置变化和停止事件。不把整个桌面程序或GStreamer渲染层直接打包进电视App。
- 上游根构建仍会编译渲染层，Android需独立维护可复查的构建入口和补丁，不能声称修改一个开关即可完成移植。`uxplay.cpp`中必要的初始化、配置和回调装配也须识别与迁移。
- 核心还链接PlayFair、DNS-SD、llhttp、libplist及OpenSSL等组件；仅提取`lib/`不等于没有传递依赖，也不等于变为宽松许可。逐文件许可、密码库版本、媒体回调与网络/停止生命周期仍须验证。

### 6.2 按实际能力兼容，不以加密失败触发无条件降级

先修正三个协议前提：

1. UxPlay README明确描述为通过“Legacy Protocol”支持AirPlay 2，并说明有缺失功能、不能保证未来iOS继续支持该旧协议。这支持把它作为兼容路线参考，但不能证明已经同时具备完整现代AP2和AP1两个镜像栈。现代AP2候选仍须单独验收。
2. 当前UxPlay接收逻辑本身使用RTSP/1.0处理`pair-setup`、`pair-verify`、`fp-setup`等请求。RTSP是会话控制协议，不能作为区分AirPlay 1/2的唯一标签；镜像的实际音视频传输还需对应的媒体通道。只有RAOP音频接收能力不构成屏幕镜像备用路线。
3. 新版iOS可能选择兼容的旧服务，但“能发现旧接收端”不保证握手失败后会透明降级或自动重连。AirPlay配对/加密由发送端和接收端协议实现处理；Wi-Fi接入加密、AP隔离、组播过滤和网络超时属于不同问题，不能统一归因为“内网加密算法”并切换协议。

默认策略为“AirPlay 2优先、自动兼容已验证旧模式”，依据引擎与真实媒体能力发布发现信息，由苹果发送端参与协商。双方支持现代AP2时优先该路径；已知只能使用旧模式时直接选择兼容配置，无需人为先制造一次失败。尚只有UxPlay兼容能力的实验构建应明确标注，不能冒充现代AP2已完成。兼容记录包含实际模式、能力配置、握手阶段、客户端系统和引擎版本，不能只记录“AirPlay 2/1”。

优先在同一引擎中协商模式；使用不同后端时，由同一个接收服务/协调器管理模式、设备身份、发现和媒体占用。切换仅在前次连接清理完成且没有其他活动/待确认会话时进行，不并行发布同名服务诱导客户端选路。

后续若具备第二组可用的镜像协议/能力配置，按以下规则处理；当前没有宣称已实现该备用路线：

| 情况 | 处理策略 |
|---|---|
| 初始发现与协商 | 广告真实已验证能力；双方支持时优先现代AP2，否则选择已验证的AP1/legacy镜像配置 |
| 明确的版本/能力不兼容，且有经过真机验证的另一组镜像配置 | 允许有上限的兼容重试；须证明发送端可重新协商，不能仅按模糊错误字符串判断 |
| 配对拒绝、PIN错误、身份/签名校验失败、完整性失败，或未分类的密钥/解密错误 | 结束当前连接并报告具体阶段；不自动降低协议、关闭认证/加密或清除信任要求 |
| 超时、AP隔离、发现失败、套接字断开 | 按网络故障诊断与原模式重连策略处理；不作为协议降级证据 |
| 只有音频可用、codec不支持或DRM内容不可显示 | 明确说明能力/内容限制，不把仅有声音记成镜像兼容成功 |

一次连接尝试最多允许一次经验证的兼容重试，所有步骤受总等待预算约束；先关闭失败会话，再用新代次重新握手，保留既定身份与接入确认要求。若需要调整发现能力，必须在没有活动会话/其他待确认连接时受控撤销和重新发布，不能虚构设备身份或任意修改能力位来反复诱导连接。

接收端不能单方面要求iPhone在同一连接中切换协议。自动重连只在特定客户端/引擎组合实测成立、且能把后续连接归入同一次重试预算时开放；否则提示用户重新选择接收设备。无法准确分类错误或控制重试次数时，不自动切换配置。兼容设置也只有在备用镜像配置实际存在并验收后才提供，不添加无实现的“AirPlay 1降级”开关。

### 6.3 分层候选与分发边界

| 优先级/用途 | 公开资料可确认 | 本项目判断 |
|---|---|---|
| 现代AP2优先原型：[rairplay](https://github.com/r4v3n6101/rairplay) | Rust库带C依赖，GPL-3.0-only；HomeKit/Legacy配对、音视频交付接口存在 | 验证现代AP2完整镜像及声音；Android宿主/媒体/密钥/兼容仍需实现和实测，未选为生产依赖 |
| C/C++镜像核心优先参考：[UxPlay](https://github.com/FDH2/UxPlay) | Legacy Protocol镜像/音频接收，项目GPLv3；协议核心与GStreamer渲染有边界 | 兼容路线及Android媒体接入参考，不代替现代AP2验收 |
| Android模块参考：[CastBay](https://github.com/weenas/castbay) | UxPlay核心、独立`:airplay`库，GPLv3，minSdk23 | 重点比较Standard API23、模块化媒体/发现与构建；未证明本项目互通 |
| Android接入参考：[android-airplay-server](https://github.com/jqssun/android-airplay-server) | 基于UxPlay的C/JNI、MediaCodec/音频输出、NSD桥和前台服务；GPLv3；构建minSdk24 | 与CastBay对比核心构建和接收链路，保留LanCast架构并核查低版本支持 |
| AP2协议参考：[shairplay-rust](https://github.com/metaneutrons/shairplay-rust) | LGPL-3.0-or-later；现代AP2音频/配对；视频仍用legacy能力配置，混合模式研究中 | 配对、音频与错误分类参考，不当作已完成现代AP2镜像的替代 |
| 补充参考：[PhairPlay](https://github.com/mazer666/PhairPlay) | Kotlin RTSP/配对/媒体代码及原生PlayFair/ALAC；顶层Apache-2.0；beta，真机验证仍在进行 | 用于对照协议与测试，不作完整现代AirPlay 2或自动降级的证据；原生组件来源/许可单独核查 |
| 备选参考：[RPiPlay](https://github.com/FD-/RPiPlay) | 树莓派镜像实现，包含H.264视频/AAC声音；整体GPLv3，引用LGPL与GPL组件 | C/C++祖先实现参考；与UxPlay存在代码渊源，不天然构成独立的AirPlay 1回退栈 |
| 音频/协议参考：[Shairplay](https://github.com/juhovh/shairplay) | README说明主要支持AirPort Express仿真；可选PlayFair握手依赖GPLv3 | 单独采用不能满足完整屏幕镜像，不作为自动降级的镜像后端 |
| Android参考：[AirplayServer](https://github.com/KqsMea8/AirplayServer) | README列出Android发现、镜像接收、MediaCodec与音频解码 | 仓库顶层许可未明确，组件来源和当前iOS兼容未完成核查 |
| 协议参考：[SteeBono/airplayreceiver](https://github.com/SteeBono/airplayreceiver) | C#/.NET镜像实现，仓库标注MIT；仍依赖原生音频库 | 核查传递许可后才可参考具体实现；不引入.NET运行时解决本功能 |

按[05 构建依赖与包体控制](05-构建依赖与包体控制.md)，各候选都要形成明确的集成/分发方案；不采购收费投屏SDK，不加入完整GStreamer/.NET运行时，也不把GPL实现无说明地复制到Apache-2.0主体。协议优先级已确定，生产库、实际发行许可、源码材料及链接组成仍须单独记录。可选模块、JNI库、独立进程或关闭开关都不自动消除分发义务。

开发依赖已固定提交、原始Git文件哈希、传递锁文件和本地修正，供可复查构建；生产准入仍需真机原型和分发验收。独立Airplay组合包使用GPL-3.0-only，不因JNI或可选变体而免除义务。移除GStreamer不改变PlayFair等组件许可。

构建可排除AirPlay模块；包含模块的构建仍保持运行时默认关闭。排除模块时不打包相关native库或服务声明，不提供可操作的开启按钮；包含但初始化失败时展示具体原因。Standard先做原型，Legacy按自身系统/codec能力单独验收；不为试验本功能提高既有接收端最低系统要求。Android Legacy产品变体与AirPlay Legacy协议模式是两个独立概念。

### 6.4 Android参考项目的具体取舍

已核对以下公开源码快照，尚未编译其工程、安装APK或执行其真机/自动化测试：

| 项目快照 | 已核查的接入点 | 对LanCast的用途 |
|---|---|---|
| CastBay：`9b04b3325b09aa426031c7a83f0cdc9f69e24b37` | 独立`:airplay`库，minSdk23、ARM32/ARM64；单独编译UxPlay核心并替换渲染/发现；当前compileSdk37 | 新增重点参考：独立库、API23适配和媒体/NSD边界；不自动升级本项目compileSdk或继承全部功能 |
| android-airplay-server：`c8defdd70d7e6a04f4f1b71d353653682d594106` | `app/src/main/cpp/CMakeLists.txt`显式编译UxPlay核心、JNI回调和DNS-SD桥；Kotlin层含`NativeBridge`、`NsdServiceManager`、`VideoRenderer`、`AudioRenderer`、`AirPlayService`；服务返回`START_NOT_STICKY` | 主要参考：与CastBay对比核心源码清单/NDK构建、发现与媒体回调、原生解码/输出及服务状态 |
| PhairPlay：`4a3948c51d7f9d050d1f7437c0bd88877ef6db0f` | `RtspHandler`、`PairingSession`、`LegacyPairSetupPin`、`MirrorStreamServer`、`TimingHandler`及对应测试；原生CMake构建PlayFair和ALAC桥；服务有`START_STICKY`恢复逻辑 | 补充参考：请求分类、配对状态、音画时钟、流生命周期和测试用例；按D15重新划定协议/平台职责，不复制整套Kotlin协议栈到UI/服务 |

android-airplay-server该快照绑定UxPlay提交`462153392f2e30937424922039ff9f0cda5e7b1a`，CastBay绑定`e3599e8c40ff1abe62146ba8a3e51c937bcf2524`，均与第6.1节检查的上游快照不同。后续固定父工程和全部子模块版本，验证各自组合；升级核心时复查回调/配置/编译补丁差异，不把最新上游直接替换进去并沿用旧互通结论。

参考时必须明确以下适配差异：

1. **服务行为**：已确认android-airplay-server与PhairPlay包含开机广播接收器；PhairPlay另有服务自动恢复。所有参考实现都只迁移用户手动启用的周期，排除开机注册和自动恢复。android-airplay-server的`START_NOT_STICKY`与本需求方向一致，但仍需验证关闭、网络变化、Activity重建和旧回调；其悬浮窗权限也不作为LanCast基本投屏前提。CastBay同样按D15生命周期重新验收。
2. **平台版本**：CastBay最低API23、android-airplay-server最低API24；PhairPlay的Fire TV最低API25、Google TV最低API29。不能照搬工程就宣称覆盖本项目全部Standard/Legacy设备；API21/22尤其需单独适配，未通过时只关闭对应AirPlay能力，不提升现有接收App最低版本。
3. **功能与依赖**：android-airplay-server包含Compose、Media3/HLS、HEVC、软件ALAC、Oboe等超出当前镜像目标的实现；PhairPlay还有Miracast/Cast等方向。只迁移已核查且当前镜像必需的接入部分。移除某个解码器前证明实际协商不需要它或已有替代；若必须新增接收端软件codec/音频库，应单列许可、包体和兼容决策，不默默扩大现有FFmpeg仅TS封装的范围。
4. **协议证据**：PhairPlay README称其栈完整，同时说明处于beta、真机验证进行中、type103缓冲音频尚未播放。所检查的`PairingSession`明确实现raw-binary（non-HomeKit）的配对路径。项目名称、README功能表或“HomeKit-style”描述，不能代替对完整现代AirPlay 2、各握手分支和回退行为的验证；同一项目内有legacy PIN处理也不证明可在加密失败后无条件降级。
5. **分发与认证**：android-airplay-server采用GPLv3；PhairPlay顶层Apache-2.0，但原生CMake明确包含来自RPiPlay的PlayFair组件，必须追溯组件许可。不能凭顶层许可证直接复制整个接收栈。参考项目的可选/默认PIN策略不能改变D15的接入确认要求；跨连接配对状态的身份绑定、作用域与清理需独立核查。

当前推荐分开验证“rairplay等现代AP2候选”与“UxPlay兼容镜像 + CastBay/android-airplay-server的Android适配”，再由LanCast服务/UI/单路协调统一管理；PhairPlay等补充协议/测试对照。先验证完整能力再决定生产后端，不因某个库语言或项目名更合意就替代AP2优先目标。最新候选、证据和排除理由见[19](19-AirPlay2开源接收调研与选型.md)。

## 7. 实施顺序与证据

分阶段交付：现代AP2与旧兼容模式的协议/分发原型 → 两种模式的完整镜像/声音及错误分类 → 手动开关与服务生命周期 → 单路仲裁、受控兼容和后台显示处理 → 系统/机型矩阵及发布。现代AP2未通过或没有可用旧镜像配置时，明确记录缺失，不把兼容单一路径或虚构降级作为完整目标成果。退出条件由[06 D15专项验收](06-开发计划与验收.md#7-d15按需airplay接收专项)统一定义。

仅发布mDNS、握手成功或协议回环都不能代替Android电视真实镜像验收。当前实现与证据在[08](08-实施进度与审阅入口.md)和[20](20-AirPlay实现与审阅指南.md)维护。0.6.0新增独立AirPlay锁文件和产品变体，已有WSS协议v2与历史验证记录保留；未通过实机验收的能力不标为机型支持已完成。

## 8. 调研依据与范围说明

核对日期：2026-10-08。上述开源链接是调研入口，不是允许浮动分支参与构建的依赖声明。

- 扩展检索、现代AP2候选和新的Android参考见[19 AirPlay 2开源接收调研与选型](19-AirPlay2开源接收调研与选型.md)；以下来源保留UxPlay及最初两个Android参考的具体源码证据。
- UxPlay固定调研快照：[README协议范围](https://github.com/FDH2/UxPlay/blob/3dbf7ceee65932154e85a2f83963d53520a799fa/README.md#L228-L238)、[核心构建及依赖](https://github.com/FDH2/UxPlay/blob/3dbf7ceee65932154e85a2f83963d53520a799fa/lib/CMakeLists.txt)、[媒体回调](https://github.com/FDH2/UxPlay/blob/3dbf7ceee65932154e85a2f83963d53520a799fa/lib/raop.h#L81-L90)、[RTSP握手处理](https://github.com/FDH2/UxPlay/blob/3dbf7ceee65932154e85a2f83963d53520a799fa/lib/raop.c#L404-L419)、[核心/渲染装配](https://github.com/FDH2/UxPlay/blob/3dbf7ceee65932154e85a2f83963d53520a799fa/CMakeLists.txt#L78-L89)。源码结构支持优先评估核心移植，但不作为Android已可运行或当前最新iOS已互通的证据。
- android-airplay-server固定快照：[说明](https://github.com/jqssun/android-airplay-server/blob/c8defdd70d7e6a04f4f1b71d353653682d594106/README.md)、[NDK构建](https://github.com/jqssun/android-airplay-server/blob/c8defdd70d7e6a04f4f1b71d353653682d594106/app/src/main/cpp/CMakeLists.txt)、[子模块](https://github.com/jqssun/android-airplay-server/tree/c8defdd70d7e6a04f4f1b71d353653682d594106/app/src/main/cpp/third_party)、[服务](https://github.com/jqssun/android-airplay-server/blob/c8defdd70d7e6a04f4f1b71d353653682d594106/app/src/main/kotlin/io/github/jqssun/airplay/service/AirPlayService.kt)、[Manifest](https://github.com/jqssun/android-airplay-server/blob/c8defdd70d7e6a04f4f1b71d353653682d594106/app/src/main/AndroidManifest.xml)、[最低系统](https://github.com/jqssun/android-airplay-server/blob/c8defdd70d7e6a04f4f1b71d353653682d594106/app/build.gradle.kts)。
- PhairPlay固定快照：[说明与未完成项](https://github.com/mazer666/PhairPlay/blob/4a3948c51d7f9d050d1f7437c0bd88877ef6db0f/README.md)、[实际配对路径](https://github.com/mazer666/PhairPlay/blob/4a3948c51d7f9d050d1f7437c0bd88877ef6db0f/app/src/main/kotlin/com/phairplay/airplay/handshake/PairingSession.kt)、[RTSP处理](https://github.com/mazer666/PhairPlay/blob/4a3948c51d7f9d050d1f7437c0bd88877ef6db0f/app/src/main/kotlin/com/phairplay/airplay/RtspHandler.kt)、[原生组件](https://github.com/mazer666/PhairPlay/blob/4a3948c51d7f9d050d1f7437c0bd88877ef6db0f/app/src/main/cpp/CMakeLists.txt)、[服务](https://github.com/mazer666/PhairPlay/blob/4a3948c51d7f9d050d1f7437c0bd88877ef6db0f/app/src/main/kotlin/com/phairplay/service/PhairPlayService.kt)、[最低系统](https://github.com/mazer666/PhairPlay/blob/4a3948c51d7f9d050d1f7437c0bd88877ef6db0f/app/build.gradle.kts)、[顶层许可](https://github.com/mazer666/PhairPlay/blob/4a3948c51d7f9d050d1f7437c0bd88877ef6db0f/LICENSE)。
- [Android前台服务](https://developer.android.com/develop/background-work/services/fgs)、[后台启动限制](https://developer.android.com/develop/background-work/services/fgs/restrictions-bg-start)、[服务类型](https://developer.android.com/develop/background-work/services/fgs/service-types)、[后台页面启动](https://developer.android.com/guide/components/activities/background-starts)：用于区分服务存活、启动权限与显示条件，实施时按实际targetSdk和系统版本复核。
- [乐播接收SDK/APK方案](https://www.lebo.cn/SdkCooperation.jsp)：公开存在厂商SDK和预装合作；本需求由用户手动启用，不以获得此类合作为前提。
- [1001 TVs的iOS投电视教程](https://www.1001tvs.com/ios-how-to-mirror-apple-ios-screen-to-tv/)：其公开流程要求两端安装并打开App，不作为苹果免安装AirPlay或系统后台特权的证据。
