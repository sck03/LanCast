# 27 Apple采集与AirPlay媒体维护（D28）

日期：2026-11-07；开发版本：0.6.13 / 21。这里记录本轮可追溯的技术复核和实现合同；构建结果见[08](08-实施进度与审阅入口.md)。

## 新技术复核与选择

| 来源与固定快照 | 本次确认的变化 | LanCast处理 |
|---|---|---|
| [rairplay](https://github.com/r4v3n6101/rairplay/commit/7a0ec4036905afe0c8de16881d85d5846dc938a7) | 最新提交仍为2026-07-20的现有基线 | 继续维护独立fork，不虚构上游版本升级 |
| [UxPlay 1.74实验版](https://github.com/FDH2/UxPlay/blob/05b0275cb1b726592700e47a6e0307544b3d3a52/README.md) | 内置mDNS实验、HLS语言处理、仅macOS可用的AWDL点对点连接；最新提交2026-11-06 | Android已经使用NsdManager；AWDL不能移植成Android能力，不引入第二套发现/接收服务 |
| [CastBay](https://github.com/weenas/castbay/blob/8834aeff1d9c59131dbd9ae7ef2e90da0ce30163/README.md) | 声称硬件HEVC/4K、HLS及更完整播放器；最新提交按Android堆大小调整HLS缓冲 | 属于UxPlay接入与产品演进；HEVC/HLS需要独立协商、解析、解码及设备验收，本轮没有公告这些能力 |
| [shairplay-rust](https://github.com/metaneutrons/shairplay-rust/blob/34ff8ad5e91b081a78fd439c5628d88ea9493f33/AP2-STATUS.md) | 更新RustCrypto依赖；视频仍说明采用不启用现代AP2位的UxPlay配置，PTP尚未连接实际播放同步 | 参考依赖收敛；不能以更新日期替代完整现代AP2镜像证据 |
| [Apple ScreenCaptureKit](https://developer.apple.com/documentation/screencapturekit) | macOS14提供过滤内容尺寸/像素比例与保持比例选项，macOS15增加明确动态范围配置 | 本轮直接采用这些API，Mac最低系统提升到15，不维护旧Mac采集分支 |

Apple原生采集与模拟AirPlay接收是两个模块。Mac使用ScreenCaptureKit向LanCast自有WebRTC发送；Android AirPlay接收仍使用独立协议库和MediaCodec。新版API不会自动赋予第三方接收器完整AirPlay能力。当前苹果发送设备仍可能选择legacy镜像握手，因此保留该线协议属于互通需求，不是兼容旧LanCast程序或旧数据格式。

官方接口依据：[contentRect](https://developer.apple.com/documentation/screencapturekit/sccontentfilter/contentrect)、[pointPixelScale](https://developer.apple.com/documentation/screencapturekit/sccontentfilter/pointpixelscale)、[preservesAspectRatio](https://developer.apple.com/documentation/screencapturekit/scstreamconfiguration/preservesaspectratio)、[captureDynamicRange](https://developer.apple.com/documentation/screencapturekit/scstreamconfiguration/capturedynamicrange)。仍使用SDR/NV12/BT.709匹配现有H.264媒体合同，不能只打开HDR选项而忽略协商、颜色和接收解码链路。

## Mac采集与构建边界

`MacCaptureConfiguration`只负责根据过滤器的点尺寸和像素倍率生成配置；`MacCapture`持有流和启动代次；`FrameSize`继续负责偶数尺寸与方向。竖屏输出可为720×1280；320×240的小窗口无需放大到1280×720；方形源使用720×720。此处是配置尺寸变化，不是实测CPU/RSS提升百分比。窗口后续改变比例时由ScreenCaptureKit保持比例，重新选择来源会重新计算输出尺寸。

采集输出注册和启动统一在异常回收作用域内，避免`addStreamOutput`失败后残留当前流引用。仍保留三个系统采集缓冲和RTC的两帧有界交付。构建脚本与产物验证共用`apple_products.PRODUCTS`的系统版本，Xcode及Swift Package同步最低macOS15。iOS ReplayKit和tvOS接收合同不变。

## AirPlay帧处理

- `streaming/processing/video.rs`独立负责128字节头部、长度校验、帧截止和媒体交付。收到首字节后，头部及正文共用5秒截止；持续零星来包不会重置预算。帧间静止等待不计入这个截止，仍受外层连接生命周期管理。帧中EOF明确为损坏输入，正常帧边界EOF才视为结束。
- 视频初始缓冲由2MiB改为64KiB，音频由256KiB改为16KiB；按实际包增长，仍保持最大帧/包限制。两个缓冲初始容量合计减少2.17MiB，不等同整个进程RSS降幅。unsupported video encryption返回错误，避免后台任务panic。
- AVC拒绝保留的三字节NAL长度和非法NAL头；先扫描所有NAL并核算Annex B输出长度，再一次性分配。转换后的大小也不能超过解码器2MiB输入合同，解决短长度前缀扩展后超限的问题。扫描仅遍历NAL边界，像素/编码正文仍只复制一次。
- 重复的相同AVC配置不再次通知Android重建解码器；真正变化仍正常交付。AAC静音标记在分配前过滤；相同四字节的合法PCM不再误删，序号锁在宿主回调前释放。

协议仍不公告HEVC、HLS、PTP或多房间。未来加HEVC应增加独立配置解析器与明确codec合同，然后扩展JNI/MediaCodec能力发现，不能仅修改能力位。

## 依赖与验证

AirPlay工作区的宿主和协议层统一使用`ed25519-dalek 3`、`sha2 0.11.1`，HKDF/X25519/ChaCha清单改为已发布的稳定系列，删除旧SHA2和Ed25519依赖链。SRP 0.7仍只有RC，保留锁定的0.7.0-rc.3；没有将它标为稳定版。主控制核心依赖图不受影响，原始上游哈希保留在`UPSTREAM.json`，发行脚本记录本地修改。

回归覆盖：静止后继续、拆包/粘包、头部/正文缓慢发送和截止、截断、超长、非法加密组合、AVC输入/输出边界、重复配置与PCM误删；原有现代SRP配对、加密控制/音视频、签名/PIN拒绝和停止回环继续执行。Mac配置测试覆盖横竖屏、Retina、小窗口、方形与非法尺寸，平台集成由macOS Actions执行。

真实iPhone/电视互通、长稳、丢包、CPU/GPU/RSS和可听音画同步仍需设备测量；CI通过只代表对应构建和自动化通过。
