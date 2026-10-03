# LanCast

Windows / Android / macOS / iOS / tvOS 局域网投屏工程，采用 Rust 控制核心、C++ 原生媒体、Kotlin 和 Swift 平台层，优先使用免费开源库。

**开发版本，尚未完成完整 v1.0 的真实设备与发行验收。** [架构与实际进度](docs/08-实施进度与审阅入口.md)区分代码、构建、自动化和实机证据；[D08](docs/09-原生媒体实现与构建决策.md)记录本轮媒体架构调整。

## 功能与模块

- 自有接收端：WSS 配对、完整 SPKI 指纹、一次邀请与电视确认；WebRTC H.264／Opus 镜像和 MP4 原文件播放。
- Windows 发送：WGC 窗口／显示器、D3D11 转换、Media Foundation 硬件 H.264、WASAPI 系统声音；独立 RTC／TS 输出模块。
- Android 发送：MediaProjection、硬件编码、合法内部声音、前台服务和授权撤销处理。
- Apple：Mac 屏幕／窗口发送与接收、iPhone／iPad ReplayKit 广播与前台接收、Apple TV 接收，详见 [D09 架构、构建与安装](docs/11-苹果客户端架构与验收.md)。
- DLNA：发现与控制、合成画面／提示音测试、用户确认档案、通过后直播、拉流监控与一次恢复；MP4 文件能力独立。
- Android Standard 使用 Media3 1.11.1；Legacy 使用系统 MediaPlayer 与固定上游 TLS 媒体桥。
- Rust domain/core/adapters/ffi 分层，编码像素不穿过控制层；媒体队列有界，停止可打断等待。

Windows 需要 Windows 10 22H2 或 Windows 11、媒体组件与可用 D3D11 硬件 H.264 编码器。Android Sender 最低 API29，Receiver Standard 最低 API23，Legacy 最低 API21。不能假定所有电视均兼容。

## 使用

自有镜像：启动电视接收端，选择局域网地址；在发送端扫描或输入地址，核对完整指纹和邀请，并在电视确认。之后选择窗口／屏幕并授权分享。

DLNA：选择本机 LAN IPv4 和电视，点击“测试 DLNA 画面和声音”；在电视确认连续彩色画面与所选声音，保存后再点击真实屏幕分享。DLNA 使用局域网 HTTP 明文，实际延迟由电视决定。测试不采集用户屏幕，HTTP 拉流本身不作为画面成功的证据。

## 构建与测试

[GitHub Actions](https://github.com/sck03/LanCast/actions/workflows/ci.yml)编译 Rust、最小 TS、Windows 原生媒体和三个 Android 调试 APK，归档产物与检查报告。

[Apple Actions](https://github.com/sck03/LanCast/actions/workflows/apple.yml)构建三个 Apple 目标并执行 Mac 集成测试。Mac 上可运行 `python3 scripts/build-apple.py --platform macos`（或 `ios` / `tvos`）；设备安装另需有效签名与广播 App Group。构建结果与真机状态见审阅入口。

```sh
python scripts/check-architecture.py
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
```

Android：JDK17、SDK36、NDK27.2.12479018，Linux 构建机设置 ANDROID_NDK_HOME：

```sh
python scripts/build-android-core.py
python scripts/build-android-media.py
cd android
./gradlew :app-receiver:assembleStandardDebug :app-receiver:assembleLegacyDebug :app-sender:assembleDebug
```

Windows：使用 VS2022 C++/Windows SDK、UCRT64 和 Python。先按工作流构建最小 TS DLL，再执行：

```powershell
$env:RUSTFLAGS = '-C target-feature=+crt-static'
cargo build --release --locked --target x86_64-pc-windows-msvc -p cast-ffi
./scripts/build-windows-deps.ps1
cmake -S windows -B windows/build -G "Visual Studio 17 2022" -A x64 -DCMAKE_PREFIX_PATH="$PWD/.cache/mbedtls-install"
cmake --build windows/build --config Release
ctest --test-dir windows/build -C Release --output-on-failure
./scripts/package-windows.ps1
```

分发时不能只复制 exe，core、RTC、TS 三个 DLL 必须保留。项目采用 [Apache-2.0](LICENSE)；依赖和发行材料见 [THIRD_PARTY.md](THIRD_PARTY.md)。没有实测的包体、延迟、音画同步或电视兼容率不作为已达标承诺。
