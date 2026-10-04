# 15 Rust依赖清单与审阅契约

日期：2026-10-04。D12续接db31e46；本轮改善构建证据和审阅工具，不改变投屏协议、应用版本或平台运行依赖。

## 范围与分层

`scripts/dependency-report.py`只负责调用Git/Cargo、读取锁文件和写报告；`scripts/dependency_inventory.py`负责纯数据转换、身份归一化和依赖图校验。后者不访问网络、不调用构建工具、不写文件，新增输出格式可以复用同一份已校验图。

输入是`cargo metadata --locked --format-version=1 --all-features`与当前`Cargo.lock`。没有指定`--filter-platform`，因此包括工作区全部feature、构建/测试及平台条件依赖。它是**源码解析清单**，不是某个APK、DLL或App实际链接内容的断言；不会据此声称已完成完整二进制SBOM、许可证法律审计或漏洞扫描。

## 生成与输出

使用Python 3.11+和仓库锁定的Cargo工具链：

```sh
python scripts/dependency-report.py
python scripts/dependency-report.py --offline --output .cache/dependency-review
python -m unittest discover -s scripts/tests
```

`--offline`要求依赖已缓存，缓存缺失会失败；默认输出到`dist/reports/`，相对`--output`以仓库根目录为基准。

| 文件 | 审阅用途 |
|---|---|
| rust-dependencies.json | 保留旧平面列表及字段，兼容原有使用者；license_file仍沿用Cargo原始值，可能是构建机路径 |
| rust-dependency-graph.json | 提交、dirty状态、锁文件SHA256、workspace根节点、已解析feature、包来源、依赖别名、normal/build/dev分类和target条件 |
| rust-source.spdx.json | SPDX 2.3源码包、crates.io purl、Cargo归档SHA256、DESCRIBES和DEPENDS_ON关系 |

新图和SPDX以`名称+版本+来源`区分同名同版本不同源包；本地包使用仓库相对路径，不暴露检出目录。包/边排序稳定，文档命名空间由规范化报告摘要生成，时间使用Git提交时间并转换为UTC。同一输入可重复生成；dirty仅表示工作树状态，不是未提交源码的内容指纹。正式审阅应以干净提交的CI产物为证据。

SPDX中的许可证来自Cargo声明，缺失为`NOASSERTION`；`licenseConcluded`始终为`NOASSERTION`，不自动裁定许可证。checksums描述Cargo锁定的下载归档，不代表编译后二进制哈希。非crates.io源保留sourceInfo，不伪造crates.io下载地址或purl。SPDX的DEPENDS_ON表示解析图包含此边；细分依赖类型和平台条件应查配套graph文件。

## 失败行为与验证

包缺少锁记录、图节点不完整、依赖边悬空、归档SHA256非法、包身份重复或本地依赖位于仓库外时，转换失败并使工作流失败。报告在所有转换成功后才写入。不要把上次成功报告当成这次失败运行的结果。

单元测试覆盖同名同版本不同源、别名、条件构建依赖、未知许可证、checksum范围、检出路径/输入顺序变化及不完整输入拒绝。Linux core和Windows现有工作流运行这些测试并上传三个报告；模块修改已加入两者的push/PR路径过滤。应用平台未改动时不需要重跑全部七个工作流，历史平台证据仍按原始提交保留。

## 后续边界

下一步应分别接入Gradle解析结果、原生源码/子模块和Apple XCFramework材料，再将其与各平台包哈希、实际链接/打包结果关联。新增采集器应返回独立数据，不向Rust转换模块添加平台SDK调用。正式发布仍需签名、完整许可材料、真机与长稳验收，见[完成矩阵](08-实施进度与审阅入口.md)和[发行依赖](05-构建依赖与包体控制.md)。
