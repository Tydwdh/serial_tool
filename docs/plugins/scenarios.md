# 串口插件场景测试 DSL

场景运行器使用**真实的** `PluginManager`、Lua 运行时、DataBus 和内存串口，不需要连接硬件。RX 注入、插件 TX、命令执行、任务取消与协议事件都走正式运行链路（`crates/extension/src/scenario.rs:23-37`）。

## 如何运行

手动运行内置示例：

```powershell
cargo run -p tool-extension --example plugin_scenario -- `
  plugins plugins/template.serial-chart/tests/serial.scenario.json
```

示例接受两个位置参数：`plugins` 根目录和场景 JSON 路径，输出 `TestRunReport` 的 JSON；只要有任一用例不是 `Passed` 就以非零码退出，适合直接用于 CI（`crates/extension/examples/plugin_scenario.rs`）。

同一份场景也已接入 `cargo test`：`crates/extension/tests/serial_scenario.rs` 中的 `bundled_serial_chart_scenario_passes` 会自动执行 `plugins/template.serial-chart/tests/serial.scenario.json`，改动该模板或运行器时无需手工运行示例。

## 场景文件结构

```json
{
  "name": "协议解析",
  "plugin_id": "template.serial-chart",
  "port": "TEST",
  "timeout_ms": 500,
  "steps": [
    { "action": "rx", "text": "{\"seq\":1,\"value\":12.3,\"target\":50.0}\n" },
    { "action": "expect_event", "topic": "protocol.template.sample", "payload": { "value": 12.3 } },
    { "action": "execute", "command": "my-plugin.send", "input": "M105" },
    { "action": "expect_tx", "text": "M105\n" },
    { "action": "cancel", "command": "my-plugin.cancel" },
    { "action": "expect_no_tx", "timeout_ms": 100 }
  ]
}
```

| 字段 | 类型 | 必填 | 默认 | 说明 |
|---|---|---:|---|---|
| `name` | string | 是 | — | 场景名，即报告中的用例名。 |
| `plugin_id` | string | 是 | — | 被启用的插件 ID（`SerialPluginScenario`，`crates/testing/src/lib.rs:84-92`）。 |
| `port` | string | 否 | `"TEST"` | 内存虚拟串口名（`crates/testing/src/lib.rs:94-95`）。 |
| `timeout_ms` | integer | 否 | `1000` | 断言步骤的默认超时（`crates/testing/src/lib.rs:97-98`）。 |
| `steps` | array | 是 | — | 顺序执行的步骤列表。 |

运行器在开始时发现插件、以 `port` 打开虚拟串口并启用 `plugin_id`，结束时停用插件并关闭串口（`crates/extension/src/scenario.rs:48-56,135-137`）。

## 步骤

步骤判别字段为 `action`，取值为蛇形名（`SerialScenarioStep`，`crates/testing/src/lib.rs:103-138`）。

| `action` | 参数 | 说明 |
|---|---|---|
| `rx` | `text` 或 `hex` | 向内存串口注入字节（`scenario.rs:67`）。 |
| `execute` | `command`，可选 `input`、`payload` | 以发送区上下文执行插件命令（`scenario.rs:70`）。 |
| `expect_tx` | `text` 或 `hex`，可选 `timeout_ms` | 在超时前等待**完全相等**的 TX 字节（`scenario.rs:86-92`）。 |
| `expect_no_tx` | 可选 `timeout_ms` | 确认指定时间窗内没有该端口的 TX（`scenario.rs:97-108`）。 |
| `expect_event` | `topic`，可选 `payload`、`timeout_ms` | 等待指定 topic 的事件（`scenario.rs:110-120`）。 |
| `wait` | `ms` | 持续驱动插件运行时指定毫秒数，不产生断言（`scenario.rs:82`）。 |
| `cancel` | `command` | 执行取消命令，语义等同宿主按钮触发（`scenario.rs:78`）。 |

- `rx` / `expect_tx` 的 `text` / `hex` 由 `ScenarioData` 承载：`bytes()` 要求**恰好提供 `text` 或 `hex` 之一**；`text` 直接取 UTF-8 字节，`hex` 忽略空白后按字节解析，奇数位报错（`crates/testing/src/lib.rs:141-168`）。
- `execute` 会发布 `plugin.command.execute`，payload 中包含 `plugin_id`、`command`、`context.send`（`input`、`target_port`、`target_port_open`）以及步骤的 `payload`（`scenario.rs:170-205`）。
- `expect_tx` 按事件 topic、端口与字节内容精确匹配；`expect_no_tx` 只判断该端口是否出现任意 TX（`scenario.rs:89-92,100-108`）。
- `expect_event` 按 topic 精确匹配；提供 `payload` 时按**对象子集**匹配（`json_contains`，`scenario.rs:116-118,262-269`）。

## 超时语义

- 每个断言步骤的 `timeout_ms` 缺省时回退到场景级 `timeout_ms`（`scenario.rs:89,100,116`）。
- `expect_tx` / `expect_event` 在超时前循环驱动插件运行时并检查事件；超时未命中即失败。
- `expect_no_tx` 在上述时间窗内等待：若出现 TX 则失败，窗口结束仍无 TX 则通过。
- 任一步骤失败会记录 `step N: ...` 并**立即停止**后续步骤，整个场景标记为 `Failed`（`scenario.rs:125-134`）。

## 虚拟串口

`port` 是一个内存虚拟串口（`TransportManager::open_virtual_serial`，`crates/extension/src/scenario.rs:50-53`）。`rx` 步骤通过 `inject_rx` 把字节注入该端口，插件在 `transport.serial.default.rx`（`crates/transport/src/lib.rs:67`）上收到；插件写入的字节表现为该端口的 `transport.serial.default.tx` 事件，供 `expect_tx` / `expect_no_tx` 观察。

## 报告

`TestRunReport` 含 `cases` 列表，每个 `TestCaseResult` 记录 `status`、`duration_ms`、`logs`、`assertions`、`error` 与 `raw_packets`（RX/TX 原始包日志，`crates/testing/src/lib.rs:19-53`，`crates/extension/src/scenario.rs:144-167`）。

## 相关文档

- [README.md](README.md)——插件开发总览。
- [manifest.md](manifest.md)——`plugin.json` 字段参考。
- [lua-api.md](lua-api.md)——`ctx.*` API 与回调。
- [spec.generated.md](spec.generated.md)——协议枚举生成物。
- [../ARCHITECTURE.md](../ARCHITECTURE.md)、[../known-gaps.md](../known-gaps.md)。
