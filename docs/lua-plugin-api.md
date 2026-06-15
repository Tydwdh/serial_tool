# Lua 实时插件 API

实时插件入口是 `main.lua`。脚本运行时会获得全局对象 `ctx`。

不同 API 是否存在取决于插件在 `plugin.json` 中声明的权限。

## ctx.plugin

插件信息，只读。通常包含：

```lua
ctx.plugin.id
ctx.plugin.name
ctx.plugin.version
```

## ctx.now_ms()

返回当前系统毫秒时间戳。

```lua
local now = ctx.now_ms()
```

## ctx.log

需要权限：`log`

```lua
ctx.log.trace(message)
ctx.log.debug(message)
ctx.log.info(message)
ctx.log.warn(message)
ctx.log.error(message)
```

## ctx.bus

需要权限：`bus`

### publish(topic, payload)

```lua
ctx.bus.publish("protocol.pid.sample", {
  t = 1,
  target = 50.0,
  actual = 49.7,
  output = 0.12
})
```

### history(topic_prefix)

读取最近最多 100 条历史事件。`topic_prefix` 可省略。

```lua
local events = ctx.bus.history("transport.serial.")
```

### wait(topic, timeout_ms)

等待一个精确 topic 事件。超时返回 `nil`。

```lua
local event = ctx.bus.wait("test.ready", 1000)
```

### subscribe(topic_prefix, timeout_ms)

等待一个 topic 前缀匹配的事件。超时返回 `nil`。

```lua
local event = ctx.bus.subscribe("transport.serial.", 1000)
```

### on(topic, callback)

注册持续事件回调。插件会保持运行状态。

```lua
ctx.bus.on("transport.serial.default.rx", function(event)
  ctx.log.info("RX: " .. tostring(event.payload))
end)
```

### off(topic)

```lua
ctx.bus.off("transport.serial.default.rx")
```

### ui.contribution.action

插件通过 `plugin.json` 的 `contributes.ui` 挂到宿主插槽后，点击动作会以普通事件送回 Lua。

```lua
ctx.bus.on("ui.contribution.action", function(event)
  local p = event.payload or {}
  if p.plugin_id ~= ctx.plugin.id then
    return
  end

  if p.action == "my-plugin.send" then
    local send = (p.context or {}).send or {}
    ctx.log.info("send input: " .. tostring(send.input))
  end
end)
```

## ctx.serial

需要权限：`serial`

### list()

```lua
local ports = ctx.serial.list()
for i = 1, #ports do
  ctx.log.info(ports[i].port_name .. " " .. tostring(ports[i].port_type))
end
```

### open(config)

```lua
ctx.serial.open({
  port_name = "COM3",
  baud_rate = 115200,
  data_bits = 8,
  stop_bits = 1,
  parity = "none",
  timeout_ms = 50
})
```

### close() / close_port(port)

```lua
ctx.serial.close()
ctx.serial.close_port("COM3")
```

### send / send_to

```lua
ctx.serial.send("hello\n")
ctx.serial.send_to("COM3", "hello\n")
```

### send_hex / send_hex_to

```lua
ctx.serial.send_hex("01 02 0A FF")
ctx.serial.send_hex_to("COM3", "01 02 0A FF")
```

### status / status_port / open_ports

```lua
local s = ctx.serial.status()
local s2 = ctx.serial.status_port("COM3")
local ports = ctx.serial.open_ports()
```

### expect(pattern, timeout_ms)

等待 RX 文本中出现指定片段。返回匹配到的文本，超时返回 `nil`。

```lua
local line = ctx.serial.expect("READY", 1000)
```

## ctx.dialog

需要权限：`dialog`

```lua
local path = ctx.dialog.open_file({
  title = "选择文件",
  filters = {
    { name = "G-code", extensions = { "gcode", "nc", "txt" } },
    { name = "所有文件", extensions = { "*" } },
  }
})
```

通过 `ctx.dialog.open_file` 选择的文件会授权给当前插件读取。

## ctx.fs

需要权限：`fs.read.user_selected`

```lua
local text = ctx.fs.read_text(path)

for line in ctx.fs.read_lines(path) do
  ctx.log.info(line)
end

for line in ctx.fs.read_lines_stream(path) do
  ctx.log.info(line)
end
```

`read_lines_stream` 使用流式逐行读取，适合较大的日志或 G-code 文件；它和 `read_lines` 一样要求文件已由宿主授权。

## ctx.ui

需要权限：`ui`

### create_chart(config)

```lua
ctx.ui.create_chart({
  id = "my-plugin.chart",
  title = "我的图表",
  topic_prefix = "protocol.my-plugin."
})
```

图表会显示匹配 topic 的 JSON payload 中所有数值字段。

### create_form(config)

```lua
ctx.ui.create_form({
  id = "my-plugin.form",
  title = "参数",
  auto_apply = true,
  fields = {
    { id = "enabled", label = "启用", kind = "checkbox", default = false },
    { id = "port", label = "端口", kind = "text", default = "COM3" },
    { id = "rate", label = "频率", kind = "select", default = "50", options = {
      { label = "10 Hz", value = "10" },
      { label = "50 Hz", value = "50" },
      { label = "100 Hz", value = "100" }
    }},
    { id = "gain", label = "增益", kind = "slider", default = 1.0, min = 0.0, max = 10.0, step = 0.1 }
  }
})
```

支持字段 `kind`：

```text
text
number
checkbox / boolean / bool
select / choice / enum / dropdown
slider / range
```

表单应用后会发布 `ui.form.changed`，payload 形如：

```json
{
  "panel_id": "my-plugin.form",
  "values": {
    "enabled": true,
    "port": "COM3",
    "rate": "50",
    "gain": 1.0
  }
}
```

监听示例：

```lua
ctx.bus.on("ui.form.changed", function(event)
  if event.payload.panel_id ~= "my-plugin.form" then
    return
  end

  local values = event.payload.values
  ctx.storage.set("port", tostring(values.port))
end)
```

### create_attitude(config)

```lua
ctx.ui.create_attitude({
  id = "my-plugin.attitude",
  title = "姿态",
  topic = "protocol.imu.attitude"
})
```

### remove_panel(panel_id)

```lua
ctx.ui.remove_panel("my-plugin.chart")
```

### get_panel(panel_id)

```lua
local panel = ctx.ui.get_panel("my-plugin.chart")
```

## ctx.timer

需要权限：`timer`

```lua
ctx.timer.after(1000, function()
  ctx.log.info("1 秒后执行")
end)

local timer_id = ctx.timer.every(100, function()
  ctx.log.info("tick")
end)

ctx.timer.cancel(timer_id)
```

## ctx.storage

需要权限：`storage`

```lua
local port = ctx.storage.get("port") or "COM3"
ctx.storage.set("port", "COM3")
local keys = ctx.storage.keys()
```

当前 storage 建议视为插件运行期存储。不要在 `v0.1-preview` 里依赖它作为长期持久化配置数据库。

## on_disable(callback)

注册插件停用时的清理函数。

```lua
on_disable(function()
  ctx.timer.cancel(timer_id)
  ctx.ui.remove_panel("my-plugin.chart")
  ctx.log.info("plugin stopped")
end)
```

## event 对象

Lua 回调中收到的 `event` 常用字段：

```lua
event.id
event.timestamp_ms
event.topic
event.source
event.direction
event.payload
event.metadata
```

典型串口 RX 事件：

```lua
ctx.bus.on("transport.serial.default.rx", function(event)
  local port = ""
  if event.metadata and event.metadata.port then
    port = tostring(event.metadata.port)
  end

  local text = tostring(event.payload)
  ctx.log.info("RX from " .. port .. ": " .. text)
end)
```
