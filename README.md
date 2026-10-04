# LanCast

Windows / Android / macOS / iOS / tvOS 局域网投屏工程，采用 Rust 控制核心、C++ 原生媒体、Kotlin 和 Swift 平台层，优先使用免费开源库。

**开发版本，尚未完成完整 v1.0 的真实设备与发行验收。** [架构与实际进度](docs/08-实施进度与审阅入口.md)区分代码、构建、自动化和实机证据；[D08](docs/09-原生媒体实现与构建决策.md)记录原生媒体架构，[D11](docs/14-控制连接生命周期与维护边界.md)记录控制层改进，[D12](docs/15-Rust依赖清单与审阅契约.md)记录依赖报告分层与源码清单。

## 功能与模块

- 自有接收端：WSS 配对、完整 SPKI 指纹、一次邀请与电视确认；WebRTC H.264／Opus 镜像和 MP4 原文件播放。
- Windows 发送：WGC 窗口／显示器、D3D11 转换、Media Foundation 硬件 H.264、WASAPI 系统声音；独立 RTC／TS 输出模块。
- Android 发送：MediaProjection、硬件编码、合法内部声音、前台服务和授权撤销处理。
- Apple：Mac 屏幕／窗口发送与接收、iPhone／iPad ReplayKit 广播与前台接收、Apple TV 接收，详见 [D09 架构、构建与安装](docs/11-苹果客户端架构与验收.md)。
- DLNA：发现与控制、合成画面／提示音测试、用户确认档案、通过后直播、拉流监控与一次恢复；MP4 文件能力独立。
- Android Standard 使用 Media3 1.11.1；Legacy 使用系统 MediaPlayer 与固定上游 TLS 媒体桥。
- Rust domain/core/adapters/ffi 分层，编码像素不穿过控制层；媒体队列有界，停止可打断等待。
- RTC 短暂断流由共享恢复策略管理：控制连接仍有效时，在固定 15 秒内最多重建三次传输，保留授权采集并过滤旧协商消息，详见 [恢复协议](docs/12-RTC恢复与协商契约.md)。
- WSS 客户端、服务端和帧收发独立封装；停止等待旧连接退出，取消配对及时释放名额并关闭对应确认框，详见[控制生命周期](docs/14-控制连接生命周期与维护边界.md)。
- 构建报告包含可追溯的Rust源码依赖图和SPDX 2.3清单，保留开源许可证声明、来源及锁文件校验值，详见[依赖审阅契约](docs/15-Rust依赖清单与审阅契约.md)。

Windows 需要 Windows 10 22H2 或 Windows 11、媒体组件与可用 D3D11 硬件 H.264 编码器。Android Sender 最低 API29，Receiver Standard 最低 API23，Legacy 最低 API21。不能假定所有电视均兼容。

## 使用

自有镜像：启动电视接收端，选择局域网地址；在发送端扫描或输入地址，核对完整指纹和邀请，并在电视确认。之后选择窗口／屏幕并授权分享。

DLNA：选择本机 LAN IPv4 和电视，点击“测试 DLNA 画面和声音”；在电视确认连续彩色画面与所选声音，保存后再点击真实屏幕分享。DLNA 使用局域网 HTTP 明文，实际延迟由电视决定。测试不采集用户屏幕，HTTP 拉流本身不作为画面成功的证据。

## 构建与测试

[GitHub Actions](https://github.com/sck03/LanCast/actions)已按 Windows、Android、macOS、iOS、tvOS 分为独立工作流，Linux 核心与原生媒体各有独立检查。每个产品支持手动设置源码分支／标签／提交、应用版本号、构建号与 Debug／Release；用法和入口见[独立构建指南](docs/13-独立平台构建与版本配置.md)。

默认应用版本与构建号统一从 [build-config.json](build-config.json) 读取。Apple 分别使用 `python3 scripts/build-macos.py`、`build-ios.py`、`build-tvos.py`，Mac 执行集成测试并生成通用包。设备安装另需有效签名与广播 App Group。

```sh
python scripts/check-architecture.py
python -m unittest discover -s scripts/tests
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
