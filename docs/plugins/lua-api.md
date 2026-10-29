# Lua 插件 ctx.* API 参考

本文是实时插件（`main.lua`）可用的 Lua API 参考。运行时在脚本执行前安装全局对象 `ctx`；每个域的安装条件见下表，未获授权的域在 `ctx` 上不存在（`ctx.serial` 为 `nil` 等）。以 `ctx` = `nil` 访问会报 Lua 错误。

- 插件入口：`plugin.json` 的 `main`（或 `live.main`），默认 `main.lua`。
- 编辑器补全用的类型存根：`plugins/.lua/hardware-workbench.lua`、`plugins/.lua/hw/*.lua`（`---@meta`）。
- 清单字段与权限声明见 [manifest.md](manifest.md)；插件协议枚举见 [spec.generated.md](spec.generated.md)。

## 权限

`ctx` 域的安装由权限控制，判据为 `has_permission`（`crates/lua_host/src/lib.rs:105`、`crates/lua_host/src/lib.rs:828`）。

| ctx 域 | 所需权限 | 安装处 |
|---|---|---|
| `ctx.plugin` | 无（总是安装） | `crates/lua_host/src/lib.rs:1002` |
| `ctx.now_ms` | 无（总是安装） | `crates/lua_host/src/lib.rs:997` |
| `ctx.commands` | 无（总是安装） | `crates/lua_host/src/lib.rs:867` |
| `on_disable` | 无（总是安装） | `crates/lua_host/src/lib.rs:1006` |
| `ctx.log` | `log` | `crates/lua_host/src/lib.rs:848` |
| `ctx.bus` | `bus` | `crates/lua_host/src/lib.rs:855` |
| `ctx.serial` | `serial` | `crates/lua_host/src/lib.rs:877` |
| `ctx.ui` | `ui` | `crates/lua_host/src/lib.rs:884` |
| `ctx.timer` | `timer` | `crates/lua_host/src/lib.rs:897` |
| `ctx.session` | `storage` | `crates/lua_host/src/lib.rs:901` |
| `ctx.dialog` | `dialog` | `crates/lua_host/src/lib.rs:905` |
| `ctx.fs` | `fs.read.user_selected` | `crates/lua_host/src/lib.rs:919` |
| `ctx.task` | `task` | `crates/lua_host/src/lib.rs:928` |
| `ctx.config` | `config` | `crates/lua_host/src/lib.rs:940` |
| `ctx.test` | `testing` | `crates/lua_host/src/lib.rs:1011` |

实时插件允许声明的权限白名单为 `LIVE_PERMISSIONS`（`crates/extension/src/spec.rs:7`）：

```text
bus, log, serial, ui, storage, timer, testing, dialog,
fs.read.user_selected, task, config
```

清单里声明白名单以外的权限会被拒绝安装。`live.permissions` 覆盖顶层 `permissions`（`crates/extension/src/manifest.rs:183`）。

脚本加载时若无任何回调/命令/定时器/未完成任务，插件立即进入完成状态并退出事件循环（`crates/lua_host/src/lib.rs:389`、`crates/lua_host/src/lib.rs:481`）。

## 通用

### ctx.plugin

插件信息，只读。安装自 `manifest_context`（`crates/extension/src/manager.rs:907`）。

| 字段 | 类型 | 说明 |
|---|---|---|
| `id` | string | 插件 ID |
| `name` | string | 显示名 |
| `version` | string | 版本 |
| `api_version` | string | 协议版本 |
| `runtime` | string | 运行时（当前仅 `lua`） |
| `root` | string | 插件根目录绝对路径 |
| `permissions` | string[] | 顶层 `permissions` 原样数组 |
| `contributes` | table | 清单 `contributes` 结构 |

### ctx.now_ms()

返回当前系统毫秒时间戳（整数）。`crates/lua_host/src/lib.rs:997`

```lua
local now = ctx.now_ms()
```

### ctx.log —— 权限 `log`

```lua
ctx.log.trace(message)
ctx.log.debug(message)
ctx.log.info(message)
ctx.log.warn(message)
ctx.log.error(message)
```

每个级别向总线发布一条 `system_log` 事件（`crates/lua_host/src/api/log.rs:18`）。

### on_disable(callback)

注册插件停用时的清理回调（全局函数，非 `ctx` 成员）。停用时宿主先取消所有任务，再调用该回调；回调抛错只记 warning（`crates/lua_host/src/api/task.rs` `call_disable`、`crates/lua_host/src/lib.rs:1006`）。

```lua
on_disable(function()
  ctx.timer.cancel(timer_id)
  ctx.ui.remove_panel("my-plugin.chart")
end)
```

## event 对象结构

`ctx.bus.on` / `ctx.bus.wait` / `ctx.bus.subscribe` 回调收到的事件表由 `event_to_lua_table` 生成（`crates/lua_host/src/convert.rs:19`）：

| 字段 | 类型 | 说明 |
|---|---|---|
| `id` | integer | 事件序号 |
| `timestamp_ms` | integer | 毫秒时间戳 |
| `topic` | string | 主题 |
| `source` | string | 来源（如 `plugin:<id>`、`transport.serial`） |
| `direction` | string | `internal` / `rx` / `tx`（`Direction` 小写，`crates/lua_host/src/convert.rs:15`） |
| `payload` | any | 空/字节/文本/JSON 映射为 nil/字符串/字符串/表 |
| `metadata` | table | 附加元数据（如串口事件含 `port`） |

> `ctx.bus.history` 返回的事件表**不含** `metadata` 字段（`crates/lua_host/src/api/bus.rs:66`）。

## ctx.bus —— 权限 `bus`

`crates/lua_host/src/api/bus.rs:24`

| 函数 | 返回 | 说明 |
|---|---|---|
| `publish(topic, payload)` | — | 发布事件 |
| `history(topic_prefix?)` | event[] | 最近最多 100 条历史（按前缀过滤，无前缀则全部） |
| `wait(topic, timeout_ms?)` | event? | 等待精确 topic，默认超时 1000ms |
| `subscribe(topic_prefix, timeout_ms?)` | event? | 等待前缀匹配，默认超时 1000ms |
| `on(topic, callback)` | — | 注册持续回调 |
| `off(topic)` | — | 注销回调 |

`publish` 拒绝 `ui.`、`transport.`、`log.` 保留前缀，要求改用对应的专用 API（`crates/lua_host/src/api/bus.rs:41`）。

`on` 注册的 topic 支持 `*` 后缀前缀匹配：`get_callback` 先精确查，再按 `topic_matches` 遍历（`crates/lua_host/src/lib.rs:592`、`crates/core/src/lib.rs:419`）：显式 `*` 结尾才前缀匹配，否则必须精确。回调存在即视为插件“有活动”，插件会保持运行。

```lua
ctx.bus.on("transport.serial.default.rx", function(event)
  ctx.log.info("RX: " .. tostring(event.payload))
end)

ctx.bus.on("protocol.demo.*", function(event)
  -- 前缀匹配
end)

ctx.bus.off("transport.serial.default.rx")
```

`wait`/`subscribe` 在顶层同步等待，超时返回 `nil`；`publish` 的保留前缀限制同样适用于插件间通信（用自定义 `protocol.*` 主题）。

## ctx.commands —— 无权限

`crates/lua_host/src/api/commands.rs:14`

| 函数 | 返回 | 说明 |
|---|---|---|
| `register(command, handler)` | — | 注册命令处理函数并广播注册事件 |
| `unregister(command)` | — | 注销并广播注销事件 |
| `list()` | string[] | 已注册命令 ID 列表 |
| `execute(command, args?)` | — | 发布一次命令执行请求（`origin = "lua.commands.execute"`） |

`plugin.json` 的 `contributes.commands` 声明宿主入口；`main.lua` 用 `register` 绑定实际处理函数。处理函数收到的 `payload` 为宿主下发的 JSON 表，常见字段（`plugins/.lua/hardware-workbench.lua` 的 `HwCommandPayload`）：

```lua
ctx.commands.register("my-plugin.send", function(payload)
  -- payload.plugin_id / payload.command / payload.contribution_id
  -- payload.slot / payload.kind / payload.args / payload.context / payload.origin
  local send = (payload.context or {}).send or {}
  ctx.log.info("send input: " .. tostring(send.input))
end)
```

命令事件仅在 `payload.plugin_id` 与本插件一致时派发（`crates/lua_host/src/lib.rs:533`）。

## ctx.serial —— 权限 `serial`

`crates/lua_host/src/api/serial.rs`。端口名比较大小写不敏感（`com3` 视同 `COM3`，`same_port` at `serial.rs:47`）。

### 端口枚举与生命周期

| 函数 | 返回 | 说明 |
|---|---|---|
| `list()` | `{port_name, port_type}[]` | 原始串口列表 |
| `devices()` | `{id, label, kind, authorized}[]` | 统一设备清单形态（`kind = "serial"`） |
| `open(config)` | — | 打开串口，记录为本插件拥有 |
| `close()` | — | 只关闭本插件通过 `open` 打开的端口 |
| `close_port(port)` | — | 关闭指定端口 |

`open` 接受字符串端口名，或表：

```lua
ctx.serial.open({
  port_name = "COM3",   -- 别名：port / port_id
  baud_rate = 115200,   -- 别名：baud
  data_bits = "8",      -- 仅接受字符串 "5"/"6"/"7"/"8"
  stop_bits = "1",      -- 仅接受字符串 "1"/"2"
  parity = "none"       -- 仅接受字符串 "none"/"even"/"odd"
})
```

字段解析见 `lua_value_to_serial_config`（`crates/lua_host/src/convert.rs:44`）；未提供时使用 `SerialConfig::default()`（115200/8/1/none）。补全存根另声明了 `timeout_ms`、`dtr`、`rts`，但当前解析器不读取这三个键，传入无效。

### 发送与状态

| 函数 | 返回 | 说明 |
|---|---|---|
| `send_to(port, text)` | — | 发送文本，**不**自动补换行 |
| `send_hex_to(port, text)` | — | 发送十六进制（如 `"01 02 0A FF"`） |
| `write_line(port, line)` | — | 发送一行文本，缺 `\n` 时自动补 |
| `status_port(port)` | `{open, port_name, baud_rate}` | 端口状态 |
| `open_ports()` | string[] | 当前已打开端口名列表 |
| `flush_rx(port)` | — | 清空本插件该端口的行缓冲 |
| `read_line(port, opts?)` | `{line?, err?}` | 读一行（见下） |

### 等待响应

| 函数 | 返回 | 说明 |
|---|---|---|
| `expect(pattern, timeout_ms?)` | string? | 等待 RX 文本命中 pattern；超时返回 nil |
| `expect_from(port, pattern, timeout_ms?)` | string? | 指定端口版 |
| `request(options)` | string? | 先订阅、再发送 `tx`、再等待 `expect` |
| `write_line_and_expect(port, line, opts?)` | `{result?, err?}` | 发送一行并等待多模式命中 |

超时默认值（`serial.rs`）：`expect` / `expect_from` / `request` 为 1000ms；`read_line` 为 5000ms；`write_line_and_expect` 为 300000ms。

`request` 选项：

```lua
local line = ctx.serial.request({
  port = "COM3",
  tx = "M115\n",
  expect = "FIRMWARE_NAME",
  timeout_ms = 1000
})
```

`write_line_and_expect` 选项：

| 键 | 默认 | 说明 |
|---|---|---|
| `timeout_ms` | 300000 | 等待上限 |
| `patterns` | `{ {pattern="^ok", action="return"} }` | 响应模式表 |
| `continue_resets_timeout` | false | 命中 `action="continue"` 时把截止时间顺延一个 `timeout_ms` |
| `delimiter` | `"\n"` | v0.3 只支持 `"\n"`，其它值报错 |
| `flush_before_send` | true | 发送前清空旧行缓冲，避免 stale ok/error 误匹配 |

```lua
local resp = ctx.serial.write_line_and_expect("COM3", "M105", {
  timeout_ms = 3000,
  patterns = {
    { name = "ok",    pattern = "ok",        action = "return" },
    { name = "busy",  pattern = "busy",      action = "continue" },
    { name = "error", pattern = "^Error",    action = "return" }
  }
})
if resp.err then
  ctx.log.warn(resp.err)          -- "timeout" / "cancelled"
elseif resp.result then
  ctx.log.info(resp.result.name .. ": " .. resp.result.line)
end
```

`resp.result` 结构为 `{ name, line, elapsed_ms }`；`elapsed_ms` 当前恒为 `0`（`crates/lua_host/src/api/task.rs` `write_expect_result`）。

### read_line

```lua
local item = ctx.serial.read_line("COM3", { timeout_ms = 1000 })
if item.line then
  ctx.log.info(item.line)
elseif item.err then
  ctx.log.warn(item.err)   -- "timeout" / "cancelled"
end
```

`read_line` 与 `write_line_and_expect` **必须在 `ctx.task.start` 的任务函数（协程）内调用**，否则除非行缓冲已就绪会抛出 `必须在 ctx.task 协程内调用`（`serial.rs` `require_task_id`、`register_read_line_apis`）。`expect` / `expect_from` / `request` 在协程内以 yield 方式等待，在顶层则同步阻塞到超时。

### 响应模式匹配规则

`match_pat`（`crates/lua_host/src/api/serial.rs:32`）同时用于 `expect`、`expect_from`、`request` 的 `expect` 与 `write_line_and_expect` 的 `patterns`。每条模式支持三种形态：

| 形态 | 匹配 | 说明 |
|---|---|---|
| `re:<regex>` | 正则 | Rust `regex` 语法，区分大小写，支持 `(?i)` 内联标志 |
| `^xxx` | 行首 | 跳过行首可选 `(...)` 前缀后 `starts_with` |
| 无前缀 | 子串 | `contains`（不做前缀剥离） |

`re:` 与 `^` 会先剥离行首形如 `(0.00000)` 的前缀（`(` 到第一个 `)`），以兼容部分 Marlin 分支在每行前加时间戳的固件；行首无 `(` 时行为不变。非法正则不匹配任何行并记录一条 warning；编译结果按模式字符串缓存（上限 256 条，`serial.rs` `compile_cached_regex`）。`patterns` 的每项为 `{ name?, pattern, action? }`，`action` 取值 `"return"`（默认）或 `"continue"`。

### 等待与消费语义

- **`expect` / `expect_from` / `request`**：在协程内由 `expect_is_due` 逐行消费该端口行缓冲直到命中；未命中的行被丢弃，命中行返回。顶层阻塞路径订阅 `serial.rx` 事件做匹配，不消费行缓冲（`serial.rs` `poll_until_match`）。
- **`write_line_and_expect`**：命中 `return` 后仅保留命中行之后尚未检查的尾部；`action="continue"` 的命中不算结果，只刷新状态文本，可选顺延超时。每 tick 最多扫描 64 行候选（`crates/lua_host/src/api/task.rs` `drain_expect_candidates`）。
- **超时/取消**：协程等待在 `now_ms > deadline_ms` 时以 `err = "timeout"` 收尾；任务被 `ctx.task.cancel` 取消时立即以 `err = "cancelled"` 唤醒（`crates/lua_host/src/api/task.rs` `wake_cancelled_wait`）。`expect*` 返回 `nil`，`read_line` 返回 `{err}`，`write_line_and_expect` 返回 `{err}`。

## ctx.dialog —— 权限 `dialog`

| 函数 | 返回 | 说明 |
|---|---|---|
| `open_file(config?)` | string? | 打开文件对话框，返回选中路径或 nil |

`crates/lua_host/src/api/dialog.rs:14`。`config` 可选 `title`、`filters`（`{name, extensions}[]`）；未声明或为空时使用“所有文件”兜底过滤器。宿主以 100ms 轮询等待结果，插件停止时立即返回 `nil`。通过该对话框选择的文件会被授权给当前插件读取（供 `ctx.fs` 使用）。

```lua
local path = ctx.dialog.open_file({
  title = "选择文件",
  filters = {
    { name = "G-code", extensions = { "gcode", "nc", "txt" } },
    { name = "所有文件", extensions = { "*" } },
  }
})
```

## ctx.fs —— 权限 `fs.read.user_selected`

`crates/lua_host/src/api/fs.rs:49`。仅能读取已由 `ctx.dialog.open_file` 授权的路径。

| 函数 | 返回 | 说明 |
|---|---|---|
| `read_text(path)` | string | 整本读取，上限 16 MiB |
| `read_lines(path)` | function | 返回迭代函数，逐次调用取一行，结束返回 nil（整本读入后迭代） |
| `read_lines_stream(path)` | function | 返回迭代函数，流式逐行读取（适合大文件），自动去掉行尾 `\r\n` |

```lua
local text = ctx.fs.read_text(path)

for line in ctx.fs.read_lines_stream(path) do
  ctx.log.info(line)
end
```

## ctx.ui —— 权限 `ui`

`crates/lua_host/src/api/ui.rs:9`。清单 `contributes.panels` 声明的静态面板由宿主自动创建/销毁；运行时 `create_*` 用于数量或结构不固定的动态面板。

| 函数 | 返回 | 说明 |
|---|---|---|
| `create_chart(config)` | — | 图表面板 |
| `create_form(config)` | — | 表单面板 |
| `create_attitude(config)` | — | 3D 姿态面板 |
| `create_gauge(config)` | — | 仪表盘面板 |
| `create_table(config)` | — | 数据表面板 |
| `remove_panel(panel_id)` | — | 移除动态面板 |
| `get_panel(panel_id)` | table? | 返回该面板最近一次创建事件的配置，无则 nil |
| `set_value(panel_id, field_id, value)` | — | 更新字段/面板状态 |
| `set_values(panel_id, values)` | — | 单事件批量更新多个字段 |
| `set_enabled(panel_id, field_id, enabled)` | — | 启用/禁用字段 |
| `set_visible(panel_id, field_id, visible)` | — | 显示/隐藏字段 |
| `set_contribution_value(contribution_id, value)` | — | 更新 `contributes.ui` 控件运行时状态 |
| `set_status(text)` | — | 向状态栏推送一条通知（Info） |
| `table_set_rows(panel_id, rows)` | — | 替换表格全部行 |
| `table_append_rows(panel_id, rows)` | — | 追加行 |
| `table_remove_rows(panel_id, row_ids)` | — | 按行 id 删除 |
| `table_clear(panel_id)` | — | 清空行 |

创建配置必须是表且含 `id`（`ensure_panel_defaults`，`ui.rs`）；`title` 缺省为 `id`。若 `id` 已在 `contributes.panels` 声明，`create_*` / `remove_panel` 会报错（宿主托管）。

各类面板的缺省（`ui.rs` `ensure_panel_defaults`）：

| kind | 缺省 |
|---|---|
| `chart` | 未给 `topic`/`topic_prefix` 时 `topic_prefix = "protocol."` |
| `form` | 未给 `fields` 时为空数组 |
| `attitude` | 未给 `topic` 时 `protocol.imu.attitude` |
| `gauge` | 未给 `topic` 时 `protocol.gauge` |

```lua
ctx.ui.create_chart({
  id = "my-plugin.chart",
  title = "我的图表",
  topic = "protocol.my-plugin.sample"   -- 或 topic_prefix = "protocol.my-plugin."
})

ctx.ui.create_gauge({
  id = "my-plugin.gauge-temp",
  title = "温度",
  topic = "my-plugin.temperature",
  min = 0, max = 100, unit = "°C", label = "传感器温度",
  zones = {
    { from = 0,  to = 60,  color = "green" },
    { from = 60, to = 80,  color = "yellow" },
    { from = 80, to = 100, color = "red" }
  }
})

ctx.ui.create_table({
  id = "my-plugin.samples",
  title = "采样记录",
  columns = { { id = "time", title = "时间", width = 120 }, { id = "value", title = "数值" } },
  sortable = true,
  selectable = true,
  max_rows = 1000
})
ctx.ui.table_append_rows("my-plugin.samples", { { id = "42", time = "12:00", value = 3.14 } })
ctx.ui.table_remove_rows("my-plugin.samples", { "42" })
```

`create_form` 的字段由面板侧 `parse_fields` 解析（`crates/panels/src/dynamic/schema.rs:65`，kind 大小写不敏感）：

| kind | 说明 |
|---|---|
| `text`（缺省） | 单行文本 |
| `number` | 数值 |
| `boolean` / `bool` / `checkbox` | 复选框 |
| `select` / `choice` / `enum` / `dropdown` | 下拉（需 `options`，项形如 `{label, value}`） |
| `slider` / `range` | 滑块（需 `min`/`max`/`step`） |
| `button` | 按钮（点击发布 `ui.form.action`，`field_id` 为按钮 id） |
| `textarea` | 多行文本 |
| `file` | 文件选择（触发宿主对话框，结果写入字段值） |
| `progress` | 进度条（0..1 或 0..100，由 `set_value` 更新） |
| `status` | 状态指示（`{ text, level }`，level：running/idle/warn/error/success） |
| `label` | 只读文本（可由 `set_value` 更新） |
| `serial` / `serial_port` / `comport` | 串口选择下拉（选项由宿主填充） |
| `separator` | 分隔线（无 id，自动生成内部 id） |

表单应用/自动应用后发布 `ui.form.changed`（`crates/panels/src/dynamic/form_render.rs:454`），payload 为 `{panel_id, values}`。按钮点击发布 `ui.form.action`，payload 含 `{panel_id, field_id, kind="button_clicked", action, values}`（`form_render.rs:107`）。

```lua
ctx.bus.on("ui.form.changed", function(event)
  if event.payload.panel_id ~= "my-plugin.form" then return end
  ctx.session.set("port", tostring(event.payload.values.port))
end)
```

仪表盘可由 `set_value` 覆盖：`set_value(panel, "value", 75.0)` 设数值，`set_value(panel, "status", "预热中")` 设状态文本（`crates/panels/src/dynamic/ingest.rs:203`）。`zones` 的 `color` 支持 `green/yellow/red/blue/cyan/purple|magenta/orange`（`crates/panels/src/gauge.rs:471`）。

## ctx.timer —— 权限 `timer`

`crates/lua_host/src/api/timer.rs:6`。

| 函数 | 返回 | 说明 |
|---|---|---|
| `after(ms, callback)` | string | 一次性定时，返回定时器 id |
| `every(ms, callback)` | string | 周期定时（间隔取 `max(ms,1)`），返回 id |
| `cancel(id)` | — | 取消定时器 |

定时器存在即视为插件有活动，插件保持运行。

```lua
local timer_id = ctx.timer.every(100, function() ctx.log.info("tick") end)
ctx.timer.cancel(timer_id)
```

## ctx.task —— 权限 `task`

`crates/lua_host/src/api/task.rs`。任务函数以 Lua 协程运行，可暂停/取消，适合长流程。

| 函数 | 返回 | 说明 |
|---|---|---|
| `start(config, fn)` | task | 启动任务；同一 `id` 已在运行时报错 |
| `cancel(id)` | — | 取消并置 `paused=false` |
| `pause(id)` | — | 仅当 `pausable` 为真时生效 |
| `resume(id)` | — | 取消暂停 |
| `list()` | task[] | 任务摘要列表 |

`config`：`id`（必需）、`title`（缺省 `id`）、`cancellable`（缺省 true）、`pausable`（缺省 true）。

任务对象方法（`create_task_methods_table`）：

| 方法 | 返回 | 说明 |
|---|---|---|
| `task:is_cancelled()` | boolean | 是否已取消 |
| `task:is_paused()` | boolean | 是否暂停 |
| `task:sleep_ms(ms)` | — | 让出协程直到延时到期（yield） |
| `task:wait_if_paused()` | — | 暂停时让出协程直到恢复（yield） |
| `task:set_progress(current, total)` | — | 设置进度 |
| `task:set_progress_percent(percent)` | — | 设置百分比（clamp 到 0..100） |
| `task:set_status(text)` | — | 设置状态文本 |
| `task:log(level, message)` | — | 追加一条任务日志（含时间戳） |

任务对象还以元表暴露内部 state 字段（`id`/`title`/`cancelled`/`paused`/`finished`/`progress_current`/`progress_total`/`progress_percent`/`status`/`error`）。`ctx.task.list()` 摘要含 `id`、`title`、`cancelled`、`paused`、`finished`、`progress_current`、`progress_total`、`progress_percent`、`status`、`error`。

```lua
ctx.task.start({
  id = "my-plugin.long-job",
  title = "长任务",
  cancellable = true,
  pausable = true
}, function(task)
  task:set_progress(0, 10)
  for i = 1, 10 do
    if task:is_cancelled() then return end
    task:wait_if_paused()
    task:sleep_ms(100)
    task:set_progress(i, 10)
  end
  task:set_status("完成")
end)
```

**协程调度语义**：`sleep_ms` / `wait_if_paused` / `read_line` / `expect` / `write_line_and_expect` 通过 `coroutine.yield` 让出；宿主事件循环每 tick 调用 `process_tasks` 收集到点/命中的任务并恢复，**每 tick 最多恢复 50 个协程**（`MAX_TASK_RESUMES_PER_TICK`，`crates/lua_host/src/api/task.rs:11`、`resume_ready_tasks`）。被取消的任务会立即以失败结果唤醒（取消优先于 sleep/等待/暂停）。停用插件时 `cancel_all_tasks` 取消全部任务并强制恢复。

## ctx.config —— 权限 `config`

`crates/lua_host/src/api/config.rs:11`。按插件 ID 分文件持久化到用户配置目录 `.../HardwareWorkbench/plugin-config/<plugin-id>.json`（`crates/extension/src/manager.rs:71`）。

| 函数 | 返回 | 说明 |
|---|---|---|
| `get(key, default?)` | any | 读配置，缺失返回 `default`（或 nil） |
| `set(key, value)` | — | 写配置 |
| `remove(key)` | — | 删除键 |
| `keys()` | string[] | 全部键 |
| `profile_list()` | string[] | profile 名列表 |
| `profile_load(name)` | table? | 读取 profile，不存在返回 nil |
| `profile_save(name, data)` | — | 保存 profile |
| `profile_delete(name)` | — | 删除 profile |

```lua
local baud = ctx.config.get("baud_rate", 115200)
ctx.config.set("baud_rate", 115200)

ctx.config.profile_save("default", { speed = 1000, accel = 500 })
local profile = ctx.config.profile_load("default")
```

## ctx.session —— 权限 `storage`

`crates/lua_host/src/api/storage.rs:5`。运行期键值存储（内存，插件停止即丢弃）。

| 函数 | 返回 | 说明 |
|---|---|---|
| `get(key)` | string? | 读 |
| `set(key, value)` | — | 写（value 必须为字符串） |
| `keys()` | string[] | 全部键 |

需要跨重启保留的数据请用 `ctx.config`。

```lua
local port = ctx.session.get("port") or "COM3"
ctx.session.set("port", "COM3")
```

## 内置 Lua 模块 hw.codec / hw.utils

宿主在沙箱收紧前预注册两个模块（`crates/lua_host/src/lib.rs` `codec::register_codec` / `register_utils`），实时插件可 `require`（`package.preload` 已冻结为只读，插件不能注入新模块）。

`hw.codec`（`plugins/.lua/hw/codec.lua`）：

| 函数 | 返回 | 说明 |
|---|---|---|
| `to_hex(bytes)` | string | 字节转十六进制 |
| `from_hex(hex)` | string | 十六进制转字节 |
| `xor8(text)` | integer | 逐字节异或 |
| `crc16_modbus(bytes)` | integer | Modbus CRC16 |
| `trim_line(line)` | string | 去行尾 |
| `split_lines(text)` | string[] | 按行拆分 |

`hw.utils`（`plugins/.lua/hw/utils.lua`）：

| 函数 | 返回 |
|---|---|
| `split(text, sep)` | string[] |
| `join(parts, sep)` | string |
| `parse_number(text)` | number? |
| `table_keys(tbl)` | any[] |
| `starts_with(text, prefix)` | boolean |
| `ends_with(text, suffix)` | boolean |
| `format_size(bytes)` | string |

```lua
local codec = require("hw.codec")
local utils = require("hw.utils")
ctx.serial.send_hex_to("COM3", codec.to_hex("hello"))
```
