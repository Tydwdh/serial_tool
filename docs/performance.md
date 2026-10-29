# 性能诊断与固定压力用例

本页面向手工性能诊断与固定压力用例采集。CI 的结构性门禁见
[CI gate](#ci-gate)。

## 运行时诊断行

桌面端有一条每 5 秒输出一次的诊断行，采样窗口有界，诊断本身不会成为新的无界分配路径。

### 桌面端

由 `crates/app/src/perf.rs` 输出，经 `log::info!`（`perf.rs:181`）。采样窗口
`SAMPLE_CAPACITY = 2048`（`perf.rs:5`）。字段：

- frame p50 / p95 / p99（ms）；
- rx / tx 字节每秒，发布事件每秒；
- Terminal / Log 的 ingest 与 render 采样；
- DataBus 平均发布耗时、subscriber backlog 事件数 / 字节数与 drop 数；
- Recorder backlog 事件数 / 字节数 / 落后秒数与写入吞吐；
- plugin 回调与 Chart 渲染采样。

Recorder backlog 字节数是用于背压判定与排查的估算值，不是文件格式记账口径。

## 固定压力用例

两条 `#[ignore]` 压力用例，从仓库根以 release 采集，将打印输出作为改动前后的记录：

```text
cargo test -p tool-panels --release pressure_50k_rows_stays_indexed -- --ignored --nocapture
cargo test -p tool-databus --release pressure_3mbps_rx_publish_and_drain -- --ignored --nocapture
```

| 用例 | 位置 | 覆盖内容 |
| --- | --- | --- |
| `pressure_50k_rows_stays_indexed` | `crates/panels/src/virtual_list.rs:360` | 50k 变高行建立虚拟行索引，按视口 + overscan 查询可见区间，断言窗口覆盖视口 |
| `pressure_3mbps_rx_publish_and_drain` | `crates/databus/src/lib.rs:754` | 向 `transport.serial.rx` 发布 1,000 条 375 字节事件，断言无损订阅收满并全部 drain |

两条用例的断言是结构性的（行索引保持完整、无损订阅不丢事件），不是计时，所以不受
运行机器快慢影响。

除固定用例外，组合场景（Terminal + Chart + Recorder + 插件）、最小化 / 恢复场景与长
无换行输入场景的运行时诊断行应单独采集。

## CI gate

`.github/workflows/ci.yml` 的 `test` job 运行压力门禁：

```text
cargo test -p tool-panels -p tool-databus -- --ignored
```

只选取这两个 crate，避免把 `crates/app` 中依赖安静调度、故意 `#[ignore]` 的计时测试
拉进 CI。上面的 `--release --nocapture` 版本仍只用于采集 before / after 数值。

## 手动计时测试

`crates/app/src/runtime/timing.rs` 中的 5 个计时测试保持 `#[ignore]`
（`timing.rs:101,112,123,134,145`），原因统一为「需要安静的本地机器，CI runner 的亚毫秒
级调度不稳定」，只在本地安静机器上手动运行：

```text
cargo test -p hardware-workbench-app spin_wait -- --ignored --nocapture
```

## 采集记录模板

每次性能重构附带一次采集，按以下字段记录：

```text
scenario:
build:
before:
after:
notes:
```
