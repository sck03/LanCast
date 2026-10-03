# 第三方组件与许可证

业务源码采用 Apache-2.0。免费开源依赖不替代编解码专利或平台发行条款。

| 组件 | 固定输入 | 用途 | 许可 |
|---|---|---|---|
| Rust crates | Cargo.lock | 控制、TLS、发现、Hyper文件/直播服务、UPnP | 依赖报告列出每个包的license/source |
| webrtc-sdk Android | 150.7871.01 | 开发期Android RTC | 包装MIT；libwebrtc BSD与附带第三方许可 |
| 定制libwebrtc目标 | 0385653a83f21acf3c916466d4088b29fe2f160b | 正式版媒体目标，尚未完成 | 按固定源与DEPS审计 |
| Media3 | 1.11.1 | Standard播放器；Legacy不包含 | Apache-2.0 |
| OkHttp | 4.12.0 | Standard pin数据源 | Apache-2.0 |
| FFmpeg | 8.0.1，SHA256见build-ffmpeg.py | 仅MPEG-TS封装 | LGPL-2.1-or-later配置；GPL/nonfree禁用 |
| nlohmann/json | 65ee68451d8eb2b5f3a30b410476ab83deb3289b | Windows JSON | MIT |
| Gradle wrapper | 8.13，分发ZIP哈希固定 | 构建 | Apache-2.0 |

FFmpeg静态链接需要在正式发行时提供许可文本、对应源代码/修改、可重链接对象与说明等适用材料。当前CI调试产物不等于完成这些义务；发布门槛记录于08。配置报告应核对实际构建结果，不仅看脚本。

Windows默认构建已移除GStreamer运行时与插件打包。windows/experimental-gstreamer保留历史自编适配代码供迁移，既不参与默认构建，也不表示新的Windows镜像已经实现。

Android上游AAR不能仅因目标源revision已写入文档就宣称可重复构建。自建产物必须明确提供lancastWebrtcAar输入及其版本、哈希、构建与许可清单；没有隐式本地替换。

来源：
- [WebRTC fork](https://github.com/webrtc-sdk/webrtc/tree/0385653a83f21acf3c916466d4088b29fe2f160b)
- [Android wrapper](https://github.com/webrtc-sdk/android)
- [FFmpeg source 8.0.1](https://ffmpeg.org/releases/ffmpeg-8.0.1.tar.xz)
- [FFmpeg法律说明](https://ffmpeg.org/legal.html)
- [Media3](https://github.com/androidx/media)
- [OkHttp](https://github.com/square/okhttp)
- [nlohmann/json](https://github.com/nlohmann/json/tree/65ee68451d8eb2b5f3a30b410476ab83deb3289b)

scripts/dependency-report.py导出解析后的Rust依赖清单；它不是包含AAR、JNI与系统库的完整二进制SBOM。
