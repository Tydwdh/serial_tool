# Lua 实时插件 API

实时插件入口是 `main.lua`。脚本运行时会获得全局对象 `ctx`。

不同 API 是否存在取决于插件在 `plugin.json` 中声明的权限。

串口插件可以使用[串口插件场景测试](serial-plugin-scenarios.md)在无硬件环境中验证 RX、TX、协议事件、超时、重试和取消。

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

## ctx.commands

命令 API 总是可用，不需要额外权限。`plugin.json` 的 `contributes.commands` 负责声明命令标题和宿主入口；`main.lua` 用 `ctx.commands.register` 注册实际处理函数。

### register(command, handler)

```lua
ctx.commands.register("my-plugin.send", function(payload)
  local send = (payload.context or {}).send or {}
  ctx.log.info("send input: " .. tostring(send.input))
end)
```

`payload` 包含 `plugin_id`、`command`、`contribution_id`、`slot` 和 `context`。

### unregister(command)

```lua
ctx.commands.unregister("my-plugin.send")
```

### list()

```lua
local commands = ctx.commands.list()
```

### execute(command, args)

发布一次命令执行请求。当前主要用于插件内部复用命令入口。

```lua
ctx.commands.execute("my-plugin.send", { source = "script" })
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

### send_to / send_hex_to

向指定端口发送文本或十六进制。十六进制字符串支持空格分隔，如 `"01 02 0A FF"`。

```lua
ctx.serial.send_to("COM3", "hello\n")
ctx.serial.send_hex_to("COM3", "01 02 0A FF")
```

### status_port / open_ports

```lua
local s = ctx.serial.status_port("COM3")  -- { open, port_name, baud_rate }
local ports = ctx.serial.open_ports()     -- 当前已打开的端口名列表
```

### expect(pattern, timeout_ms)

等待 RX 文本中出现指定片段。返回匹配到的文本，超时返回 `nil`。

```lua
local line = ctx.serial.expect("READY", 1000)
```

多串口同时打开时，建议指定端口：

```lua
local line = ctx.serial.expect_from("COM3", "ok", 1000)
```

### request(options)

发送请求并等待指定响应。宿主会先注册监听，再发送数据，避免响应太快导致漏接。

```lua
local line = ctx.serial.request({
  port = "COM3",
  tx = "M115\n",
  expect = "FIRMWARE_NAME",
  timeout_ms = 1000
})
```

### flush_rx(port)

清空当前插件针对指定端口的行缓冲。

```lua
ctx.serial.flush_rx("COM3")
```

### write_line(port, line)

向指定端口发送一行文本；如果 `line` 没有以 `\n` 结尾，宿主会自动补上。

```lua
ctx.serial.write_line("COM3", "M105")
```

### read_line(port, options)

读取指定端口的一行文本。该 API 会让出 Lua 协程，必须在 `ctx.task.start` 创建的任务函数里调用。

```lua
local item = ctx.serial.read_line("COM3", { timeout_ms = 1000 })
if item.line then
  ctx.log.info(item.line)
elseif item.err == "timeout" then
  ctx.log.warn("read timeout")
end
```

### write_line_and_expect(port, line, options)

发送一行文本，并等待一组响应模式中的某一项命中。该 API 也必须在 `ctx.task.start` 的任务函数里调用。
等待期间已经检查但未命中任何模式的响应行会被消费；命中响应之后尚未检查的行会保留给后续读取。原始串口事件仍会正常发布到终端和数据总线。

```lua
local resp = ctx.serial.write_line_and_expect("COM3", "M105", {
  timeout_ms = 3000,
  patterns = {
    { name = "ok", pattern = "ok", action = "return" },
    { name = "busy", pattern = "busy", action = "continue" },
    { name = "error", pattern = "^Error", action = "return" }
  }
})

if resp.err then
  ctx.log.warn(resp.err)
elseif resp.result then
  ctx.log.info(resp.result.name .. ": " .. resp.result.line)
end
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

清单 `contributes.panels` 中的静态面板由宿主自动管理。下面的 `create_*` 仅用于运行时数量或结构不固定的动态面板；若传入已在清单声明的 ID，宿主会拒绝并给出错误。

### create_chart(config)

图表显示匹配 topic 的 JSON payload 中所有数值字段。两种订阅方式：

- `topic_prefix`：订阅该前缀下所有 topic（适合一组相关信号）。
- `topic`：精确订阅单个 topic（推荐，避免前缀下其他数据混入）。

```lua
-- 精确订阅单个 topic
ctx.ui.create_chart({
  id = "my-plugin.chart",
  title = "我的图表",
  topic = "protocol.my-plugin.sample"
})

-- 或前缀订阅
ctx.ui.create_chart({
  id = "my-plugin.chart",
  title = "我的图表",
  topic_prefix = "protocol.my-plugin."
})
```

payload 中除 `t`/`time`/`timestamp` 外的数值字段都会成为一条曲线，`t` 用作 X 轴。

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
text            单行文本
number          数值
checkbox / boolean / bool   复选框
select / choice / enum / dropdown   下拉选择（需提供 options）
slider / range  滑块（需提供 min/max/step）
button          按钮（点击发布 ui.form.changed，field_id 即按钮 id）
textarea        多行文本
file            文件选择（点击触发宿主对话框，结果写入字段值）
progress        进度条（0..1 或 0..100，由 set_value 更新）
status          状态指示（{ text, level }，level: running/idle/warn/error）
label           只读文本（可由 set_value 更新）
serial          串口选择下拉（选项由宿主自动填充）
separator       分隔线（无 id）
```

`select` 字段的 `options` 形如 `{ { label = "10 Hz", value = "10" }, ... }`。

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
  ctx.session.set("port", tostring(values.port))
end)
```

### create_attitude(config)

3D 姿态指示器，订阅 `{ roll, pitch, yaw }`（度）的 topic。

```lua
ctx.ui.create_attitude({
  id = "my-plugin.attitude",
  title = "姿态",
  topic = "protocol.imu.attitude"
})
```

### create_gauge(config)

仪表盘，订阅 `{ value }` 的 topic。可定义色区（green/yellow/red 对应 正常/预警/异常），状态文本自动按当前值所在色区生成，也可由 `set_value(panel, "status", text)` 运行时覆盖。

```lua
ctx.ui.create_gauge({
  id = "my-plugin.gauge-temp",
  title = "温度",
  topic = "my-plugin.temperature",
  min = 0,
  max = 100,
  unit = "°C",
  label = "传感器温度",
  zones = {
    { from = 0,  to = 60,  color = "green" },
    { from = 60, to = 80,  color = "yellow" },
    { from = 80, to = 100, color = "red" }
  }
})
```

`zones` 的 `color` 支持：`green`/`yellow`/`red`/`blue`/`cyan`/`purple`/`orange`。建议 zones 覆盖整个 `min..max` 区间，否则未覆盖段会显示为灰色轨道。

### create_table(config)

创建插件控制的数据表。`columns` 定义列；行是带稳定 `id` 的对象。

```lua
ctx.ui.create_table({
  id = "my-plugin.samples",
  title = "采样记录",
  columns = {
    { id = "time", title = "时间", width = 120 },
    { id = "value", title = "数值" }
  },
  sortable = true,
  selectable = true,
  max_rows = 1000
})

ctx.ui.table_set_rows("my-plugin.samples", rows)
ctx.ui.table_append_rows("my-plugin.samples", { { id = "42", time = "12:00", value = 3.14 } })
ctx.ui.table_remove_rows("my-plugin.samples", { "42" })
ctx.ui.table_clear("my-plugin.samples")
```

用户选择行时会发布 `ui.table.selection_changed`，payload 包含 `panel_id`、`row_id` 和完整 `row`。

### remove_panel(panel_id)

```lua
ctx.ui.remove_panel("my-plugin.chart")
```

### get_panel(panel_id)

```lua
local panel = ctx.ui.get_panel("my-plugin.chart")
```

### set_value / set_enabled / set_visible

用于由插件主动更新表单字段或面板状态。

```lua
-- 表单字段
ctx.ui.set_value("my-plugin.form", "status", {
  text = "运行中",
  level = "running"
})
ctx.ui.set_enabled("my-plugin.form", "start_btn", false)
ctx.ui.set_visible("my-plugin.form", "advanced", true)

-- 仪表盘：field_id = "value" 设数值，"status" 设状态文本
ctx.ui.set_value("my-plugin.gauge-temp", "value", 75.0)
ctx.ui.set_value("my-plugin.gauge-temp", "status", "预热中")

-- label 字段也可用 set_value 更新文本
ctx.ui.set_value("my-plugin.form", "samples", "919")
```

多个字段应使用单事件批量更新，减少队列压力和中间状态闪烁：

```lua
ctx.ui.set_values("my-plugin.form", {
  progress = 100,
  status = { text = "完成", level = "success" }
})
```

### set_contribution_value(contribution_id, value)

更新 `contributes.ui` 声明的控件运行时状态（如 toggle / progress / label）。

```lua
ctx.ui.set_contribution_value("my-plugin.send.progress", 0.6)
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

## ctx.task

需要权限：`task`

`ctx.task` 用于可暂停、可取消的长任务，适合发送文件、等待设备响应或执行多步骤流程。任务函数以 Lua 协程运行，因此可以调用 `task:sleep_ms`、`ctx.serial.read_line`、`ctx.serial.write_line_and_expect` 这类会等待的 API。

```lua
ctx.task.start({
  id = "my-plugin.long-job",
  title = "长任务",
  cancellable = true,
  pausable = true
}, function(task)
  task:set_progress(0, 10)

  for i = 1, 10 do
    if task:is_cancelled() then
      task:set_status("已取消")
      return
    end

    task:wait_if_paused()
    task:set_status("步骤 " .. tostring(i))
    task:sleep_ms(100)
    task:set_progress(i, 10)
  end

  task:set_status("完成")
end)
```

任务控制：

```lua
ctx.task.pause("my-plugin.long-job")
ctx.task.resume("my-plugin.long-job")
ctx.task.cancel("my-plugin.long-job")

for _, task in ipairs(ctx.task.list()) do
  ctx.log.info(task.id .. " " .. tostring(task.status))
end
```

任务对象常用方法：

```lua
task:is_cancelled()
task:is_paused()
task:wait_if_paused()
task:sleep_ms(ms)
task:set_progress(current, total)
task:set_progress_percent(percent)
task:set_status(text)
task:log(level, message)
```

## ctx.config

需要权限：`config`

`ctx.config` 是插件持久配置。配置按插件 ID 分文件保存到用户配置目录：

```text
%APPDATA%\HardwareWorkbench\plugin-config\<plugin-id>.json
```

基本读写：

```lua
local baud = ctx.config.get("baud_rate", 115200)
ctx.config.set("baud_rate", 115200)
ctx.config.remove("baud_rate")

for _, key in ipairs(ctx.config.keys()) do
  ctx.log.info("config key: " .. key)
end
```

配置 profile：

```lua
ctx.config.profile_save("default", {
  speed = 1000,
  accel = 500
})

local profile = ctx.config.profile_load("default")
local names = ctx.config.profile_list()
ctx.config.profile_delete("old")
```

## ctx.session

需要权限：`storage`

```lua
local port = ctx.session.get("port") or "COM3"
ctx.session.set("port", "COM3")
local keys = ctx.session.keys()
```

`ctx.session` 是插件运行期存储，适合保存当前运行中的轻量状态。需要跨应用重启保留的数据，优先使用 `ctx.config`。

## on_disable(callback)

注册插件停用时的清理函数。

```lua
on_disable(function()
  ctx.timer.cancel(timer_id)
  ctx.ui.remove_panel("my-plugin.chart")
  ctx.log.info("plugin stopped")
end)
```

## 回放分析器

当插件声明了 `replay` 配置（见 `plugin-manifest.md`），其 `replay.lua` 在回放期间运行，可使用 `ctx.replay` 和以下回调。普通实时插件不会用到这些 API。

### ctx.replay

```lua
ctx.replay.emit("protocol.demo.sample", { value = 1.0 })  -- 向回放流注入事件
ctx.replay.log("分析进度...")                              -- 写入回放日志
local ev = ctx.replay.current_event()                     -- 当前输入事件
```

### on_replay_begin(session) / on_replay_event(event) / on_replay_end()

```lua
function on_replay_begin(session)
  -- session.start_ms / end_ms / event_count
  ctx.log.info("回放开始，共 " .. tostring(session.event_count) .. " 条")
end

function on_replay_event(event)
  -- 逐条处理回放事件
end

function on_replay_end()
  ctx.log.info("回放结束")
end
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
