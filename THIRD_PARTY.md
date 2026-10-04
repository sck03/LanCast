# 第三方组件与许可证

业务源码采用 Apache-2.0。免费开源依赖不替代编解码专利或平台发行条款。

| 组件 | 固定输入 | 用途 | 许可 |
|---|---|---|---|
| Rust crates | Cargo.lock | 控制、TLS、发现、Hyper文件/直播服务、UPnP | 依赖报告列出每个包的license/source |
| webrtc-sdk Android | 150.7871.01 | 开发期Android RTC | 包装MIT；libwebrtc BSD与附带第三方许可 |
| Android定制libwebrtc目标 | 0385653a83f21acf3c916466d4088b29fe2f160b | 原定源构建目标，尚未完成；当前使用固定上游AAR | 按固定源与DEPS审计 |
| libdatachannel | 9e6a13abbb6846c003d817d0387b6706466e2b03 | Windows RTC传输 | MPL-2.0 |
| libopus | 5ec2f3c915d0529b94a3a302969c673531654824 | Windows Opus编码 | BSD及随附notices |
| Mbed TLS | 947808ba53faf09c526f575f5635e2e86472ba5d | Windows DTLS | Apache-2.0许可分支 |
| libjuice/libsrtp/usrsctp/plog | libdatachannel固定子模块 | ICE/SRTP/SCTP/日志 | BSD/BSD/BSD/MIT，保留各自许可 |
| Media3 | 1.11.1 | Standard播放器；Legacy不包含 | Apache-2.0 |
| OkHttp | 4.12.0 | Standard pin数据源 | Apache-2.0 |
| FFmpeg | 8.0.1，SHA256见build-ffmpeg.py | MPEG-TS封装、Windows H.264 SPS/VUI元数据修正 | LGPL-2.1-or-later配置；GPL/nonfree禁用，无软件编码器/解码器 |
| nlohmann/json | 65ee68451d8eb2b5f3a30b410476ab83deb3289b | Windows JSON | MIT |
| Gradle wrapper | 8.13，分发ZIP哈希固定 | 构建 | Apache-2.0 |
| LiveKitWebRTC Apple XCFramework | 150.7871.02，SHA256见apple_build.py | Apple H.264/Opus/ICE/DTLS/SRTP，无云服务依赖 | WebRTC BSD及附带第三方许可；上游二进制，非本仓库源构建 |
| XcodeGen | 2.44.1，SHA256见apple_build.py | 构建时生成Xcode工程，不进入运行包 | MIT |
| Apple系统框架 | 具体SDK版本见build-report.json | SwiftUI/ScreenCaptureKit/ReplayKit/AVFoundation/Metal | 系统SDK条款；不是开源替代库 |

FFmpeg静态链接需要在正式发行时提供许可文本、对应源代码/修改、可重链接对象与说明等适用材料。当前CI调试产物不等于完成这些义务；发布门槛记录于08。配置报告应核对实际构建结果，不仅看脚本。

Windows默认构建使用自有WGC/MF/WASAPI模块与libdatachannel。已移除不参与构建的旧GStreamer实现和安装脚本；选择理由见D08，旧代码可从Git历史恢复。

Android上游AAR不能仅因目标源revision已写入文档就宣称可重复构建。自建产物必须明确提供lancastWebrtcAar输入及其版本、哈希、构建与许可清单；没有隐式本地替换。

来源：
- [WebRTC fork](https://github.com/webrtc-sdk/webrtc/tree/0385653a83f21acf3c916466d4088b29fe2f160b)
- [Android wrapper](https://github.com/webrtc-sdk/android)
- [FFmpeg source 8.0.1](https://ffmpeg.org/releases/ffmpeg-8.0.1.tar.xz)
- [FFmpeg法律说明](https://ffmpeg.org/legal.html)
- [Media3](https://github.com/androidx/media)
- [OkHttp](https://github.com/square/okhttp)
- [nlohmann/json](https://github.com/nlohmann/json/tree/65ee68451d8eb2b5f3a30b410476ab83deb3289b)
- [LiveKitWebRTC固定发行版](https://github.com/livekit/webrtc-xcframework/releases/tag/150.7871.02)
- [XcodeGen固定发行版](https://github.com/yonaskolb/XcodeGen/releases/tag/2.44.1)

`python scripts/dependency-report.py`导出兼容的Rust依赖列表、带提交/锁文件SHA256及条件依赖的图、SPDX 2.3源码清单；Linux/Windows工作流随依赖产物归档。详见[依赖审阅契约](docs/15-Rust依赖清单与审阅契约.md)。它不是包含AAR、JNI与系统库的完整二进制SBOM，也不自动裁定许可证。
