# 串口插件场景测试

场景运行器使用真实的 `PluginManager`、Lua runtime、DataBus 和内存串口，不需要连接硬件。RX 注入、插件 TX、命令、任务取消和协议事件均走正式运行链路。

运行内置示例：

```powershell
cargo run -p tool-extension --example plugin_scenario -- `
  plugins plugins/template.serial-chart/tests/serial.scenario.json
```

场景文件基本结构：

```json
{
  "name": "协议解析",
  "plugin_id": "my-plugin",
  "port": "TEST",
  "timeout_ms": 500,
  "steps": [
    { "action": "rx", "hex": "01 02 03" },
    { "action": "execute", "command": "my-plugin.send", "input": "M105" },
    { "action": "expect_tx", "text": "M105\n" },
    { "action": "expect_event", "topic": "protocol.temperature", "payload": { "value": 25 } },
    { "action": "cancel", "command": "my-plugin.cancel" },
    { "action": "expect_no_tx", "timeout_ms": 100 }
  ]
}
```

支持的动作：

- `rx`：向内存串口注入 `text` 或 `hex`。
- `execute`：执行插件命令，并提供发送区 `input` 上下文。
- `expect_tx`：在超时前等待完全相等的 TX 字节。
- `expect_no_tx`：确认指定超时窗口内没有 TX，适合取消/暂停测试。
- `expect_event`：等待 topic；`payload` 使用对象子集匹配。
- `wait`：持续驱动插件运行时指定毫秒数。
- `cancel`：执行取消命令，语义上等同宿主按钮触发。

重试行为通过连续多个 `expect_tx` 验证；取消后接 `expect_no_tx` 可以确认后台任务没有继续下发。命令返回非零代表场景失败，适合直接用于 CI。
