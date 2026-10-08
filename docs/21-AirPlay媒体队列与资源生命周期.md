# 21 AirPlay媒体队列与资源生命周期

日期：2026-10-08；D17维护，开发版本0.6.2 / 10。对应程序提交、构建结果与测试证据见[08](08-实施进度与审阅入口.md)。本文件负责接收队列、解码资源及播放线程的实现合同；协议、启用周期和单路仲裁仍见[20](20-AirPlay实现与审阅指南.md)。

## 修复依据

旧队列仅用队首时间戳检查跨度。格式配置的时间戳为0，配置尚未消费时，后续媒体可以绕过时长限制；乱序时间戳也可能让实际跨度超过预算。新增回归在修改前分别复现了这两种错误。

旧播放器把`create → configure → start`放在`also`内，只有全部成功才赋给工作线程的资源变量。中途失败时，外层`finally`拿不到刚创建的解码器或音频输出。现用`configureOrRelease`在初始化作用域内持有资源，失败立即释放，成功才转移所有权；释放异常保留为原始失败的附加信息。

原音视频线程在队列为空时每2ms检查一次。现在无解码器时等待数据通知；解码器运行后，最长每10ms检查待输出缓冲，新输入会立即唤醒队列等待。AAC解码结果直接通过`ByteBuffer`写入`AudioTrack`，去掉每个输出块的临时PCM数组和复制；`AudioTimestamp`按音频线程复用。

## 模块与所有权

| 模块 | 负责范围 | 维护边界 |
|---|---|---|
| [MediaQueue](../android/receiver-contracts/src/main/kotlin/dev/lancast/receiver/contracts/MediaQueue.kt) | 帧数/字节/时间跨度、等待通知、原子关闭 | 纯Kotlin/JDK，不依赖Android、网络或解码器；保留FIFO消费顺序 |
| [ResourceSetup](../android/airplay-receiver/src/main/kotlin/dev/lancast/airplay/ResourceSetup.kt) | 初始化成功转移所有权、失败回收 | 模块内的内联函数，可用普通资源替身测试；不创建平台线程 |
| [MediaPipeline](../android/airplay-receiver/src/main/kotlin/dev/lancast/airplay/MediaPipeline.kt) | 解码、调度、音频焦点和工作线程 | 每个解码器及音频输出只由所属工作线程操作；生命周期线程关闭队列并等待退出 |
| [android_products](../scripts/android_products.py) | 按所选产品/Debug或Release执行测试 | 接收产品运行队列契约；包含AirPlay时追加资源初始化测试 |

`ResourceSetup`没有引入运行依赖。后续新增媒体格式应复用队列和资源所有权合同；具体解码/输出能力放在平台媒体适配中。若改为异步解码回调，须重新验证输出缓冲归还、关闭等待和迟到回调，不能把平台API放入契约层。

## 队列边界

| 队列 | 最多条目 | 编码数据字节上限 | 正时间戳最大跨度 |
|---|---:|---:|---:|
| 视频 | 24 | 8MiB | 300ms |
| 音频 | 64 | 1MiB | 500ms |

格式配置占条目及其实际字节，但时间戳0不参与媒体跨度。负时间戳和负字节数被拒绝。跨度取所有已排队正时间戳与新条目的最大值减最小值；两个单调队列以摊销O(1)维护边界，支持重复、乱序时间戳和出队后的预算回收，不对编码帧重新排序。

`offer`不等待空位，任一上限超出即返回失败；原生调用方据此结束会话并报告`MEDIA_BACKPRESSURE`。不会静默丢弃已编码帧后继续播放破损的GOP。容量是待消费编码队列的限制，还需计入工作线程当前条目、解码器及AudioTrack内部缓冲，不能据此声称整个进程只占9MiB。

`take`可中断地等待数据，`poll(timeoutMillis)`使用单调等待预算。`close`在同一锁内禁止入队、清空条目及时间索引、唤醒全部等待者；重复关闭安全。`clear`清除当前数据但不会重新打开已关闭队列。这样关闭与原生回调并发时，迟到入队不会残留媒体数组。

## 播放与释放

停止及播放失败先关闭两条队列，再由各线程的`finally`回收资源；显式关闭还会中断调度等待并等待两个工作线程退出。解码器的`stop`失败仍尝试`release`。音频格式变化回收旧输出，同一次播放只申请一次音频焦点，退出时释放已取得的焦点。

AAC输出缓冲在`AudioTrack.write`完成、过期丢弃、停止或异常后通过`finally`归还解码器。写入采用非阻塞接口，保留原有音频时钟与停止检查；PCM原始音频只包装已有数组。平台编解码驱动的真实行为和声音同步仍需设备验证。

## 自动化与实机验收

队列回归覆盖：格式在队首、乱序/重复时间戳、极值移除、字节/帧数边界、整数边界、清空、等待/超时/中断、关闭唤醒与入队竞争。另用一万次固定随机操作对照直接计算窗口的参考模型。资源测试覆盖初始化成功、部分初始化失败、清理失败保留原始异常。

在已配置Android SDK的构建机执行：

```sh
cd android
./gradlew :receiver-contracts:test :airplay-receiver:testDebugUnitTest
# Release产品构建会对应执行 :airplay-receiver:testReleaseUnitTest
```

GitHub仍通过`python scripts/build-android.py`执行原生库准备后的组装、lint、契约测试、产物校验及打包。测试报告随Android审阅材料上传。JVM测试不调用真实MediaCodec/AudioTrack，不作为真机性能证据。

实机继续按[06](06-开发计划与验收.md)执行：无音轨等待、AAC-LC/AAC-ELD/PCM、44.1/48kHz、反复旋转/格式切换、解码器初始化失败、停止时仍有排队媒体及音频焦点丢失。分别记录线程/解码器数量、分配与RSS、CPU、可听同步和两小时长稳；本轮不填写未经测量的提升百分比或端到端延迟。
