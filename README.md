# LanCast

局域网投屏：Android Kotlin 接收/发送、Rust 安全控制核心、C++ Windows 原生客户端；支持自有 WebRTC 镜像、MP4 文件直放和 DLNA 视频发送。

当前版本 **0.2.0-alpha，首次产品代码构建验证中**。不得将源码实现或 CI 产物等同于电视真机性能与正式发布验收。旧分析文档正在依据实际实现校正；请以本次提交的源码和 CI 结果审阅。

- Android 接收：Standard API23+ / Legacy API21+，共享业务代码、分别使用 Media3 1.11.1 / 1.8.1。
- Android 发送：API29+，MediaProjection 与允许的内部声音采集，前台通知可停止。
- Windows：Win32 + GStreamer 1.26.10，WGC / D3D11 / Media Foundation H.264 / WASAPI loopback。
- 控制：WSS、完整 SPKI 指纹绑定、ECDSA 挑战、新设备必须在电视确认。当前信任仅在进程会话内有效。
- 受限电视：DLNA 只提供视频播放；AirPlay/Miracast 提供系统操作说明。

## 构建

[GitHub Actions](https://github.com/sck03/LanCast/actions/workflows/ci.yml) 自动运行 Rust 检查、构建三个 Android APK 和 Windows x64 客户端，保留构建报告。初期 APK 使用调试签名；没有上架签名密钥。

```sh
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

Android：JDK17、SDK36、NDK27.2.12479018，设置 ANDROID_NDK_HOME 后运行 `python scripts/build-android-core.py`，再进入 android 执行 Gradle assemble 任务。Windows 的完整步骤由 `.github/workflows/ci.yml` 与 `scripts/install-gstreamer.ps1` 固定。

## 模块

| 目录 | 职责 |
|---|---|
| core | 协议、身份、会话、WSS、发现、受限文件服务、DLNA、C ABI/JNI |
| android/control-bridge | 平台线程与 Rust 事件桥接 |
| android/media-webrtc | 音视频采集、WebRTC、资源释放 |
| android/player-* | 独立播放接口与两个 Media3 依赖变体 |
| android/app-* | 发送/接收 UI 和生命周期 |
| windows/src | Win32 UI 与独立 GStreamer 媒体适配器 |
| scripts、.github/workflows | 可复现构建、验证与打包 |

许可证见 [LICENSE](LICENSE) 与 [THIRD_PARTY.md](THIRD_PARTY.md)。
