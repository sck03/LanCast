# 22 HTTP生命周期与直播分发

日期：2026-10-09；D18维护，开发版本0.6.3 / 11。当前验证和源码范围见[08](08-实施进度与审阅入口.md)。本文件负责共享HTTP服务与DLNA直播订阅的所有权和分发合同；AirPlay媒体队列仍见[21](21-AirPlay媒体队列与资源生命周期.md)。

## 修复依据

原HTTP服务只保留监听任务，连接使用独立`tokio::spawn`。取消监听后的返回没有等待连接释放；直接中止监听任务还会留下连接。每连接的取消令牌在正常完成后也不会自动取消，相关请求工作可能继续存活。两项新增回环回归已在旧实现复现失败。

现在监听服务通过`JoinSet`拥有连接任务，同时最多四个；优先回收已完成任务后再接受新连接。撤销和监听错误均中止并等待连接任务回收；宿主直接丢弃/中止监听任务时，任务集也会中止其子任务。每连接通过取消守卫在正常返回、错误或被中止时通知关联工作。

## 模块与所有权

| 模块 | 职责 | 边界 |
|---|---|---|
| [http_server](../crates/cast-adapters/src/http_server.rs) | IP过滤、最多四连接、TLS握手、HTTP解析、任务所有权和连接取消 | 不解释文件范围、直播门控或上游身份策略；不依赖Runtime/UI |
| [live](../crates/cast-adapters/src/live.rs) | 资源撤销、TS入口、私有订阅、容量/时间限制和分发 | `Subscription`拥有接收通道、起播门控和取消；仅编码后的TS进入此模块 |
| [ts](../crates/cast-adapters/src/ts.rs) | TS校验、PAT/PMT及实际SPS/PPS/IDR起播解析 | 不拥有网络任务或平台采集资源 |
| [media_http](../crates/cast-adapters/src/media_http.rs) / [legacy_bridge](../crates/cast-adapters/src/legacy_bridge.rs) | 文件Range/HEAD与固定HTTPS上游桥 | 共用连接生命周期，各自保留授权和响应策略 |

不新增crate或运行依赖。后续增加HTTP资源时，复用服务的任务所有权与请求取消令牌，资源自身处理授权、数据策略和撤销；不能在平台UI重新实现网络停止流程。正常`serve`返回是连接任务回收屏障；外部强制中止只发出子任务中止，调用方若需要确定完成应走资源撤销并等待监听退出。

## 分发与预算

TS入口仍验证每个数据块和65424字节上限；有活跃订阅者才创建一次`Bytes`分发分配。所有订阅者各自等待合法起点，门控通过后释放其解析状态，直接使用共享数据，不再经过`data.to_vec()`和重复逐包校验。跨FFI的输入寿命没有改变，入口需要的复制仍保留。

| 约束 | 实现与行为 |
|---|---|
| 单订阅排队 | 16块，最多1,046,784字节；满队列关闭该订阅，不阻塞生产者 |
| 起播总时限 | 8秒；在读取队列前检查，已到期的订阅不能靠排队数据继续起播 |
| 不可解码起播窗口 | 最多2秒后重置解析，另保留解析器8MiB上限 |
| 已排队数据 | 超过2秒拒绝，避免持续补播过时画面 |
| 稳定播放等新数据 | 最多2秒，停止可立即打断等待 |
| 订阅关闭 | 丢弃响应/订阅即取消关联连接；撤销后拒绝保留迟到订阅 |

队列预算不等于进程内存预算，起播重组、当前块、Hyper/TLS/内核缓冲另外存在。共享分配测试证明两个稳定订阅者持有相同数据地址，没有实测CPU/RSS或端到端延迟提升百分比。

## 验证与进度

七项新增Rust回归验证：正常结束取消及12次连接复用；四连接上限和释放后重连；正常停止/强制中止释放请求所有权；两个稳定订阅共享分配；积压数据不能延长起播截止；过时数据与等待撤销；丢弃及迟到订阅不留活跃读者。原有慢读者隔离、文件Range/HEAD/撤销和Legacy固定身份回环继续运行。

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
cargo check -p cast-ffi --no-default-features --locked
cargo check -p cast-ffi --no-default-features --features legacy --locked
python scripts/check-architecture.py
python -m unittest discover -s scripts/tests
```

GitHub继续使用仓库现有平台构建脚本和`github-ci.py`。Linux原生媒体工作流对真实合成H.264/AAC TS执行中途加入、分片及HTTP/1.0和HTTP/1.1传输，再由FFmpeg解码；平台构建确认共享Rust改动能进入各客户端。准确运行、产物及清理结果集中于08，防止状态在多个文档漂移。

实机仍按[06](06-开发计划与验收.md#9-d18-http生命周期与直播分发)检查连续seek、慢电视断连重拉、网络切换、重复启停、资源数量与两小时稳定性。自动化不替代电视画面、可听同步、签名发行或实际性能验收。
