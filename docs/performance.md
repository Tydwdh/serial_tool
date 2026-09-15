# Performance diagnostics and fixed pressure runs

The application reports a five-second diagnostic line through `log` containing:

- frame p50/p95/p99;
- RX/TX bytes per second and published events per second;
- Terminal/Log ingest and render samples;
- DataBus publish time, subscriber backlog and drops;
- Recorder queued events/bytes, seconds behind and write throughput;
- plugin callback and dynamic-panel render samples.

The samples use a bounded rolling window, so diagnostics do not become another
unbounded allocation path. Recorder backlog bytes are estimates intended for
backpressure decisions and triage, not file-format accounting.

## Fixed pressure runs

Run these from the repository root and capture the printed output as the
before/after record for performance changes:

```text
cargo test -p tool-panels --release pressure_50k_rows_stays_indexed -- --ignored --nocapture
cargo test -p tool-databus --release pressure_3mbps_rx_publish_and_drain -- --ignored --nocapture
```

The first run exercises 50k variable-height rows with viewport-only queries.
The second sends 1,000 deterministic 375-byte RX events through DataBus and
drains them losslessly. The runtime diagnostic line should be captured
separately for the combined Terminal+Chart+Recorder+plugin scenario,
minimize/restore scenario, and long no-newline input scenario.

## CI gate

`ci.yml` 的 `test` job 会运行这两个门禁：

```text
cargo test -p tool-panels -p tool-databus -- --ignored
```

它们的断言是结构性的——虚拟行索引必须保持 50k 行且视口查询非空、无损订阅必须
收满 1,000 条事件——而不是计时，因此 debug 构建即可作为回归门禁，单次约 2ms。
上面的 `--release --nocapture` 版本仍然只用于采集 before/after 数值。

`crates/app/src/runtime/timing.rs` 里的 5 个计时测试继续保持 `#[ignore]`：它们依赖
亚毫秒级调度，CI runner 上不稳定，只在本地安静机器上手动运行。

Every performance refactor should include:

```text
scenario:
build:
before:
after:
notes:
```
