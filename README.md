# LanCast

局域网投屏项目，采用 Rust 控制核心、Kotlin Android 与 C++ 原生媒体模块，优先免费开源依赖。

**当前是实施中的开发版本，不是完整 v1.0 成品。** [架构与实际完成进度](docs/08-实施进度与审阅入口.md)逐项区分源码、自动化、平台构建和实机验收。目标包含 WebRTC 镜像、DLNA 桌面直播和 MP4 原文件播放；不能把目标当成已支持能力。

## 当前工程

- Rust 四层：cast-domain、cast-core、cast-adapters、cast-ffi；无逐帧像素穿过 Rust 控制层。
- WSS 配对、一次邀请、完整 SPKI 指纹与电视确认；Hyper 文件服务、Range、目标 IP/token、撤销。
- DLNA 独立控制与连续 TS HTTP 发布，有界队列、慢读关闭和实际起播内容检查。
- Android Standard 使用 Media3 1.11.1；Legacy 使用系统 MediaPlayer 和 Rust TLS 媒体桥，移除旧 Media3。
- Android 镜像使用固定上游 AAR 过渡；新增 DLNA 原生采集/TS 试验代码，CI 构建、lint 与 ABI 检查通过，实机验收未完成。
- Windows 默认构建为原生控制/文件客户端，不再要求 GStreamer。**WGC/MF/WASAPI/libwebrtc 自建媒体后端未完成，屏幕分享按钮禁用。**

## 构建与验证

[GitHub Actions](https://github.com/sck03/LanCast/actions/workflows/ci.yml)分别运行 Rust、最小 FFmpeg/C++、Android 与 Windows 构建。某一作业通过不代表其他平台通过；调试 APK 不是正式发布包。

```sh
python scripts/check-architecture.py
cargo test --workspace --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo fmt --all --check
```

Android（Linux构建机）：JDK17、SDK36、NDK27.2.12479018、CMake/Ninja/make；设置 ANDROID_NDK_HOME 后：

```sh
python scripts/build-android-core.py
python scripts/build-android-media.py
cd android
./gradlew :app-receiver:assembleStandardDebug :app-receiver:assembleLegacyDebug :app-sender:assembleDebug
```

Windows：VS2022 C++ 工具链，先执行 `cargo build --release --locked --target x86_64-pc-windows-msvc -p cast-ffi`，再用 `cmake -S windows -B windows/build -G "Visual Studio 17 2022" -A x64` 和 `cmake --build windows/build --config Release`。

最小 TS 库：`python scripts/build-ffmpeg.py --prefix .cache/ffmpeg-host`；随后 CMake 指定绝对 FFMPEG_ROOT。脚本校验固定源 SHA256、禁止GPL/nonfree、只开启MPEG-TS mux，不附带FFmpeg命令行或软件编解码器。

## 审阅顺序

1. [产品路线](docs/01-产品范围与技术决策.md)与[模块契约](docs/02-模块架构与接口契约.md)。
2. [实际实施进度](docs/08-实施进度与审阅入口.md)，关注未完成的 Windows 媒体后端、定制 WebRTC、设备 Probe 与真机测试。
3. 对应提交的 CI 日志、测试和依赖报告；旧文档位于 docs/archive，仅供历史参考。

项目采用 [Apache-2.0](LICENSE)；依赖和发行约束见 [THIRD_PARTY.md](THIRD_PARTY.md)。没有通过实测的延迟、音画同步、长稳或“兼容全部电视”承诺。
