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

Every performance refactor should include:

```text
scenario:
build:
before:
after:
notes:
```
