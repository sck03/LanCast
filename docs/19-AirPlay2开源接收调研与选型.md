# 19 AirPlay 2开源接收调研与选型

日期：2026-10-08。本章保留[D15](18-AirPlay按需接收设计与接入方案.md)的前期公开仓库检索/源码调研；当时没有编译候选或执行真机投屏。后续0.6.0已集成固定rairplay/PlayFair并补修正、Android宿主及自动化，当前状态以[20实施记录](20-AirPlay实现与审阅指南.md)和[08进度](08-实施进度与审阅入口.md)为准。下文其余候选的上游功能声明仍不是LanCast实测证据。

## 1. 结论与优先级

产品目标确定为：**AirPlay 2优先，向下兼容本项目需要的AirPlay 1/legacy屏幕镜像及伴随声音**。运行时仍默认关闭，由用户打开接收App后启用。协议优先级与具体开源库的选择分开管理。

检索发现更近更新的Android实现和支持现代配对的协议库，但在本轮检查的项目中，尚未找到可凭公开证据确认同时满足“现代AirPlay 2镜像/声音完整、Android现成接入、旧版兼容、稳定性已充分验证”的全面更优替代品。不能用项目名称或较新的提交时间代替这些条件。

前期按以下顺序推进原型；0.6.0据此选择第一项作为开发后端并锁定源码，生产准入仍保留真机与分发门槛：

1. **现代AirPlay 2协议原型优先验证rairplay**：已存在HomeKit/Legacy配对及音视频交付接口，符合AP2优先的研究方向；仍需补Android宿主、发现、媒体输出、生产密钥管理和实机证据。它是Rust库并带C依赖，不能称为已经完成的Android接收App。
2. **UxPlay继续作为C/C++镜像核心的优先参考和兼容路线**：Android接入对比CastBay与android-airplay-server；前者有API23独立库，后者有明确的UxPlay/JNI实现。它们可以改善Android工程接入，但不自动补齐UxPlay未实现的现代AP2能力。
3. **shairplay-rust补充现代AP2协议研究**：其AP2音频和配对值得参考，但屏幕镜像仍使用旧兼容配置，不能当作已完成现代AP2视频的替代。
4. **PhairPlay及opentvcast补充请求处理、时钟与测试用例**。源码路径与README中的“完整AirPlay 2”描述分别核对；不直接复制整个应用或认证策略。

若现代AP2原型没有通过，不将已有legacy镜像原型标为“AP2优先目标已完成”；保留可用兼容路线并如实记录未完成项。也不为了显示“AP2”而同时启动两个争用端口、身份或播放区域的接收器。

## 2. 检索方法与比较口径

通过GitHub仓库搜索检索`airplay2 receiver`、`airplay receiver mirroring`、`airplay android receiver`、`airplay receiver language:Rust`、`airplay receiver HomeKit`、`airplay2 receiver language:C++`，按更新情况筛选，再检查README、状态文档、构建文件、关键协议路径和平台边界。结合此前的UxPlay、android-airplay-server、PhairPlay及RPiPlay参考。

比较至少区分：

- **协议**：实际配对模式、会话控制加密、媒体流类型、发现能力与旧版兼容，不仅比较名称中的“AirPlay 2”。
- **媒体任务**：屏幕镜像与伴随声音、独立音频、URL/HLS播放、照片、多房间分别记录；音频接收器不能替代电视镜像接收器。
- **集成**：库还是完整应用、Android最低系统/ABI、硬件解码和显示接口、服务生命周期与必要依赖。
- **证据**：README声明、代码存在、构建/自动化、上游声称的真机结果、本项目实测各自标注。没有执行过的测试不写成通过。
- **免费与许可**：源码公开且有明确许可才进入集成评估；顶层MIT/Apache不代表所有原生组件采用同一许可。免费二进制或仅非商业许可不能直接等同可自由集成的开源库。

本项目的AP2验收聚焦苹果原生屏幕镜像及伴随声音；多房间、type103缓冲音乐、Apple Home电视遥控等额外功能不自动纳入首版目标，但缺失时也不能宣称“完整支持AirPlay 2所有功能”。

## 3. 重点候选与证据

| 项目 | 本轮核对的更新/快照 | 实际发现 | 推荐用途 |
|---|---|---|---|
| [rairplay](https://github.com/r4v3n6101/rairplay) | 2026-07-20；`7a0ec4036905afe0c8de16881d85d5846dc938a7` | GPL-3.0-only；Rust库，编译部分C代码；HomeKit/Legacy模块、控制通道编解码、音视频接口存在；无现成接收App/播放器 | 现代AP2优先原型候选，尚未证明Android完整互通 |
| [shairplay-rust](https://github.com/metaneutrons/shairplay-rust) | 2026-09-11；`2fb72b30b658b33f106165a5b6fbd941919bdbe5` | LGPL-3.0-or-later；AP2音频、HomeKit配对和加密控制；README/状态文档明确视频使用UxPlay兼容位，AP2+video混合仍研究中 | AP2协议/音频参考，不替换镜像主线 |
| [CastBay](https://github.com/weenas/castbay) | 2026-10-07；`9b04b3325b09aa426031c7a83f0cdc9f69e24b37` | GPLv3；UxPlay核心，独立`:airplay`库；库与App最低API23，ARM32/ARM64；原生媒体与Android发现适配 | 新增重点Android模块参考，尤其匹配Standard API23；不是新的AP2协议核心 |
| [android-airplay-server](https://github.com/jqssun/android-airplay-server) | 2026-08-23；`c8defdd70d7e6a04f4f1b71d353653682d594106` | GPLv3；UxPlay + JNI + MediaCodec/音频输出 + NSD + 服务；最低API24 | 保留Android接入的主要参考，与CastBay按模块比较 |
| [PhairPlay](https://github.com/mazer666/PhairPlay) | 2026-06-14；`4a3948c51d7f9d050d1f7437c0bd88877ef6db0f` | beta；README称AP2已实现，也承认真机验证进行中、缓冲音频未播放；已查PairingSession为raw-binary non-HomeKit路径；含原生PlayFair | 握手/同步/测试对照，不能据其名称判定完整现代AP2 |
| [opentvcast](https://github.com/sueichen/opentvcast) | 2026-10-08；`12ce22722bfaa1dcc853ced0303cea0071b449c4` | 更近更新的Android Kotlin项目，拆分airplay/core/platform等模块；仍说明真机验证进行中、type103不播放 | 补充模块与测试组织参考，尚无充分证据取代主线 |

### rairplay：更接近现代AP2研究，但需要产品化

[HomeKit路由](https://github.com/r4v3n6101/rairplay/blob/7a0ec4036905afe0c8de16881d85d5846dc938a7/src/pairing/homekit/mod.rs)有`pair-setup`/`pair-verify`处理，另有[Legacy模块](https://github.com/r4v3n6101/rairplay/tree/7a0ec4036905afe0c8de16881d85d5846dc938a7/src/pairing/legacy)和[控制通道codec](https://github.com/r4v3n6101/rairplay/blob/7a0ec4036905afe0c8de16881d85d5846dc938a7/src/pairing/homekit/codec.rs)。[VideoStream接口](https://github.com/r4v3n6101/rairplay/blob/7a0ec4036905afe0c8de16881d85d5846dc938a7/src/playback/video.rs)交付带类型和时间戳的解密后编码数据，可作为Android媒体桥的研究入口。

这些结构还不能证明“现代配对 + 实际镜像 + 伴随声音 + AP1回退”已经在目标iPhone/电视组合中闭环。其[README](https://github.com/r4v3n6101/rairplay/blob/7a0ec4036905afe0c8de16881d85d5846dc938a7/README.md)明确要求集成者提供网络入口、设备实现、存储和播放；默认Keychain使用固定开发身份，不适合生产。[Cargo清单](https://github.com/r4v3n6101/rairplay/blob/7a0ec4036905afe0c8de16881d85d5846dc938a7/Cargo.toml)还有多个rc/pre密码库版本，集成时必须重新核查依赖稳定性与锁文件。生产接收端使用自己的设备密钥和可信设备存储，不带入示例身份。

若原型通过，协议实现放在独立适配层，经窄接口对接Kotlin服务和原生媒体；不把Rust领域/业务核心变为解码器，也不为了这个库引入第二套应用UI。

### shairplay-rust：“支持AP2”和“AP2视频闭环”有明确区别

它的[README](https://github.com/metaneutrons/shairplay-rust/blob/2fb72b30b658b33f106165a5b6fbd941919bdbe5/README.md)列出AP2缓冲音频、SRP/HomeKit、ChaCha20-Poly1305控制通道和AP1运行模式，但视频仍标为实验功能。[AP2-STATUS](https://github.com/metaneutrons/shairplay-rust/blob/2fb72b30b658b33f106165a5b6fbd941919bdbe5/AP2-STATUS.md)说明其iOS18视频成果使用UxPlay旧能力配置，不启用该现代AP2配置；AP2+video混合及部分时钟/事件能力还未完成接通。

因此，可用它学习现代AP2配对和错误分类，不把“可选择AP1/AP2音频”当作“完整镜像可无缝切换AP2/AP1”的证据。

### CastBay：新的Android模块参考

[库构建](https://github.com/weenas/castbay/blob/9b04b3325b09aa426031c7a83f0cdc9f69e24b37/airplay/build.gradle.kts)确认最低API23，支持ARMv7a/ARM64；[原生构建](https://github.com/weenas/castbay/blob/9b04b3325b09aa426031c7a83f0cdc9f69e24b37/airplay/src/main/cpp/CMakeLists.txt)单独构建UxPlay协议，替换GStreamer渲染及发现后端，接Android媒体/NSD。结构比直接搬整个桌面程序更贴近LanCast；已有独立库也值得与android-airplay-server的App内接入比较。

当前快照使用compileSdk37，而LanCast基线为36；不能因此自动升级本项目工具链。它绑定的UxPlay子模块为`e3599e8c40ff1abe62146ba8a3e51c937bcf2524`，也不同于android-airplay-server的`462153392f2e30937424922039ff9f0cda5e7b1a`。父工程、核心与桥接补丁必须成组核对。

其README声称测试过部分电视/车机，也说明Mac Music应用有UxPlay不支持的FairPlay类型；这些都是上游提供的信息，未在本项目复测。自带模拟发送器明确跳过网络、配对和加密，不能用该模拟器证明原生iOS互通或AP2加密路径成功。API21/22、默认关闭、手动启用、认证和完整关闭仍按D15重新验收。

## 4. 其他检索结果为何不直接替换

| 项目 | 本轮确认的边界 |
|---|---|
| [GoogleTVAirPlay](https://github.com/mkatadev/GoogleTVAirPlay) | 更新较近，AirPlay核心仍基于UxPlay；另有独立`:homekit`模块，以`_hap._tcp`提供电视配件/遥控。能加入Apple Home不等于AirPlay媒体控制通道已升级为完整现代AP2；Android TV12+和其他依赖也不匹配我们的全部设备 |
| [Shairport Sync](https://github.com/mikebrady/shairport-sync) | C实现可构建AP2或经典AP1音频，README明确不支持AirPlay视频/照片；适合音频/现代协议研究，不能替代镜像接收 |
| [openairplay/airplay2-receiver](https://github.com/openairplay/airplay2-receiver) | 实验性Python实现，重点是HomeKit配对、实时/缓冲音频；未作为完整Android镜像方案验证，不能引入Python运行时当作已解决接收链路 |
| [rsplay](https://github.com/iDescriptor/rsplay) | 较新的Rust镜像库，项目标注WIP，参考UxPlay/RPiPlay旧镜像模式；缺少足够的成品/互通证据。README中的法律保证也不代替组件许可核查 |
| [AirPlay-Windows](https://github.com/moieric11/AirPlay-Windows) | README明确从UxPlay移植，主要改善Windows原生显示/声音/发现；不是新的完整AP2后端，也不是Android接收模块 |
| [xenos1337/AirPlayServer](https://github.com/xenos1337/AirPlayServer) | Windows接收器及无界面适配，有音视频/队列设计可参考；现有资料不足以证明在现代AP2完整性和Android集成上优于重点候选 |

## 5. AirPlay 2优先、AirPlay 1兼容的落地合同

1. 先证明候选在现代AP2模式下能完成对应的配对、控制、真实屏幕镜像和伴随声音；只有能力位、HomeKit配件服务、空播放后端或仅音频成功均不通过。
2. 对已验证双方都支持的模式，优先AP2；对只支持旧模式的设备，选择已验证的AP1/legacy镜像。`Legacy`字符串不自动代表全部AirPlay 1功能，支持矩阵需注明实际路径与系统版本。
3. 优先使用一个引擎中可正确协商的多个模式；若用不同后端，由一个服务与会话协调器选择，发现/设备身份/媒体占用保持一致。不同时发布两个同名服务制造“自动兼容”。
4. 协议/能力不兼容时可按D15第6.2节进行一次受控重试；认证、PIN、身份、完整性、未知密钥/解密错误不触发无条件降级。重连可能需要用户重新选择接收设备，不能承诺iOS必然透明切换。
5. 不把关闭认证、关闭加密、仅播放声音或反复改变设备身份作为兼容成功；网络发现/路由问题走独立诊断。
6. 只有旧兼容模式通过时，准确记录为兼容能力，现代AP2目标继续保持未完成。默认关闭、手动开启、不随开机启动等产品行为不随选库改变。

原型和发布条件由[06 D15专项验收](06-开发计划与验收.md#7-d15按需airplay接收专项)维护，实际进度由[08](08-实施进度与审阅入口.md)记录。此处调研本身不证明候选可用；0.6.0后续实现、修正和测试应按20及其对应源码提交复查。
