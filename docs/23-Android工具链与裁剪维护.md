# 23 Android工具链与裁剪维护

日期：2026-10-13；决策D19；默认版本0.6.4 / 12。构建运行、实际源码、体积与清理记录以[08](08-实施进度与审阅入口.md)为准。

## 提示来源与升级边界

旧Android构建737479a成功，但`gradle/actions/setup-gradle`在结束步骤报告Gradle8.13过时。它是工具链维护提示，不表示APK编译失败。依据[Gradle稳定版本接口](https://services.gradle.org/versions/current)、[9.8.1兼容表](https://docs.gradle.org/9.8.1/userguide/compatibility.html)和[AGP9.4兼容表](https://developer.android.com/build/releases/agp-9-4-0-release-notes)，本轮固定Gradle9.8.1、AGP9.4.1、Kotlin2.4.21；JDK仍为17。AGP9.4要求Gradle至少9.6.0及Build Tools36.0.0。

Gradle发行包SHA-256为`dce76f55f8e251a3a1f130eb120f30b3d271de2b76c9b0729d316b5a1b6dc01f`；wrapper JAR为`3b8a25775a69158b5ad2b1d17a80a88dc7b40a352a73eb27d06c612a7ce68e98`。版本不在CI动态选择latest，也不关闭过期版本提示。Kotlin官方完整兼容表的上限可能落后于Gradle最新稳定版，因此本组合还必须通过项目实际Debug/Release构建，不能仅靠版本号判断兼容。

按照[内置Kotlin迁移指南](https://developer.android.com/build/migrate-to-built-in-kotlin)，Android模块删除`org.jetbrains.kotlin.android`插件与重复JVM工具链设置，由AGP按Java17目标编译；纯JVM `receiver-contracts`仍使用Kotlin JVM插件。额外变体目录通过`AndroidSourceSet.kotlin`注册。Media3实现归入实际的`player-standard`模块，删除旧`player-media3`跨目录引用。

AGP9默认只为被测试构建类型（通常Debug）创建单元测试任务。本工程保留Debug/Release各自测试合同，因此显式设置`android.onlyEnableUnitTestForTheTestedBuildType=false`；每次脚本仍只运行所选配置的测试。Gradle完整弃用诊断与根项目Problems报告随CI归档，便于后续版本升级审阅。

## 维护边界

| 位置 | 职责 |
|---|---|
| `android/gradle/libs.versions.toml` | AGP、Kotlin、SDK、Build Tools、Media3及desugaring的统一版本来源 |
| Gradle wrapper | 固定Gradle发行包、网络超时及SHA-256；标准启动脚本 |
| `scripts/android_toolchain.py` | 读取并校验稳定版本，向Actions提供SDK安装参数 |
| `scripts/android_products.py` | 产品身份、角色、构建/lint及所需JVM测试任务 |
| `scripts/android_dependencies.py` | 兼容Gradle8/9的项目依赖记法，检查实际解析的Media3版本和可选模块隔离 |
| `scripts/android_artifacts.py` | 实际APK身份/服务/库/ABI/ELF、JNI签名和R8映射校验；归档与体积组成 |
| `CodecBuffers` | 纯ByteBuffer范围复制与编码配置拼接，不依赖Android、UI或传输 |
| `DlnaCapture` / `PlaybackAudio` / `RtcPeer` | 平台资源生命周期与对应媒体通路；不改变共享控制协议 |

新增产品继续扩展产品清单；新增JNI入口同时更新所属模块保留规则和验证合同。不得把可选AirPlay模块反向引入基础产品，也不为几个共享函数增加通用插件框架。

NDK版本也由版本目录提供给Actions和Android模块，避免AGP默认NDK版本与已安装版本不同导致跳过原生符号处理。依赖检查兼容Gradle9带引号的`project ':name'`输出，并检查版本选择箭头右侧的实际解析值；类似项目名、报告说明文字或仅请求了正确版本均不能冒充通过。

## 资源回收与复制

- `DlnaCapture`创建视频编码器后即登记所有权，`configure`、Surface创建或`start`失败仍由关闭路径释放。
- AAC线程在`finally`释放自己使用的编码器和录音器；两秒关闭等待超时只影响等待方，不撤销I/O线程的清理责任。线程尚未启动时，由编排线程释放部分初始化资源。每个已取出的编码输出缓冲均在`finally`归还。
- `PlaybackAudio`序列化开始/停止，初始化失败即时释放；仅在启动成功后公布录音器，释放前清空引用。`RtcPeer`关闭时再次停止录音，并隔离每项清理异常。
- RTC首帧观察只提交一次任务；当前轨道首帧成功后移除观察器，旧轨道/协商回调仍检查身份与代次。
- 视频关键帧的配置前缀和有效载荷复制到同一个数组，避免先复制帧再拼接的第二个数组。音频和视频共用范围复制函数，保持原始ByteBuffer位置/界限不变；配置拼接不再将每个字节装箱成列表。

这减少了明确的分配和重复任务，但没有据此声明真实CPU、RSS或端到端延迟下降百分比。

## Release裁剪与审阅

Sender、Standard、Legacy、AirPlay的Release开启R8与资源裁剪。控制JNI、TS JNI/回调及AirPlay JNI/回调分别由所属模块保留规则管理；固定上游WebRTC的Java/JNI接口整体保留，避免在没有源构建证据时猜测可删除接口。

打包使用Android SDK自带`apkanalyzer dex code`检查实际方法声明和完整描述符，防止名称仍出现在字符串池却已被删除/改名的误判。检查覆盖项目自有JNI入口与回调；不能替代所有上游JNI运行路径的实机测试。Release必须生成非空`mapping.txt`，其SHA-256写入产品清单；映射、R8删除清单与完整配置进入独立报告产物，不进入用户APK。

报告同时记录APK大小以及解压DEX、原生库、资源字节数，比较必须使用相同Release配置、相同ABI组合。R8优化Java/Kotlin和Android资源，不宣称裁剪了上游WebRTC的原生库。Debug仍是开发签名；Release仍需自行签名后安装。

## 验证与剩余工作

自动验证包括Python产品/打包/JNI拒绝错误回归、四项ByteBuffer JVM回归、既有接收合同/队列/初始化测试、四种产品Debug/Release组装和lint，以及真实APK的身份/角色/ABI/16KB对齐和JNI检查。测试用例、运行及样本来源见08。

物理设备仍需覆盖：录屏授权撤销与立即停止、编码器配置/启动失败、声音读取被停止唤醒、首帧后持续镜像与重新协商、Release下的TS回调/AirPlay回调、API21 Legacy安装及播放、ARM32/ARM64声音同步和两小时长稳。测量设备型号、系统、程序SHA、空闲/播放CPU与RSS、帧率及停止后恢复情况；构建成功不填写真机完成状态。
