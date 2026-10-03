# 第三方组件与许可证

本项目业务源码采用 Apache-2.0。优先使用免费、公开源码的组件；“免费库”不代表所有编解码专利或商店分发条件自动豁免。

| 组件 | 固定基线 | 用途 | 许可证 / 来源 |
|---|---|---|---|
| Rust crates | Cargo.lock | 协议、TLS、发现、文件与 UPnP | 各包 Cargo metadata；主要 MIT/Apache-2.0/BSD/ISC |
| webrtc-sdk Android | 150.7871.01 | Android H.264/Opus/WebRTC | SDK MIT；libwebrtc BSD-3-Clause 及其第三方 notices |
| Media3 | 1.11.1 / 1.8.1 | Standard / Legacy 文件播放 | Apache-2.0 |
| OkHttp | 4.12.0 | 绑定 SPKI 的媒体 HTTPS | Apache-2.0 |
| GStreamer | 1.26.10 MSVC x64 | Windows WGC/D3D11/MF/WASAPI/WebRTC | 核心与大部分插件 LGPL-2.1-or-later；随分发包附带对应 notices |
| nlohmann/json | 65ee68451d8eb2b5f3a30b410476ab83deb3289b | Windows JSON | MIT |
| Gradle wrapper | 8.13 | 构建 | Apache-2.0；third_party/gradle-LICENSE.txt |

GStreamer 以可替换 DLL 与插件动态链接，不静态合入程序。二进制分发须保留库许可证与来源，允许用户替换 LGPL 库。当前打包脚本带入官方运行时插件集，正式发布前应完成逐插件 SBOM 与许可证筛选；不可将“bad/ugly”插件包名称直接当成许可证判断。

来源：
- https://github.com/webrtc-sdk/android/tree/main/Licenses
- https://github.com/webrtc-sdk/webrtc/tree/0385653a83f21acf3c916466d4088b29fe2f160b
- https://gitlab.freedesktop.org/gstreamer/gstreamer/-/tree/1.26.10
- https://gstreamer.freedesktop.org/data/pkg/windows/1.26.10/msvc/
- https://github.com/androidx/media
- https://github.com/square/okhttp
- https://github.com/nlohmann/json/tree/65ee68451d8eb2b5f3a30b410476ab83deb3289b

未复制 Sunshine、Moonlight、UxPlay 等 GPL 项目的业务源码；历史调研是设计参考。Android 使用上游二进制，不能把指定源码提交自动视为该二进制的可复现构建证明；可选源码构建流程另有构建清单。
