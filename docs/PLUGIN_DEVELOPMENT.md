# 插件开发总览

硬件调试工作台的插件是一组放在 `plugins\<plugin-id>\` 下的文件。主程序启动或刷新插件列表后，会扫描每个子目录中的 `plugin.json`。

最小插件：

```text
plugins\my.plugin\
  plugin.json
  main.lua
```

如果插件希望支持“只录原始串口数据，然后回放时重新解析出图表”，再增加：

```text
  replay.lua
```

插件不需要重新编译主程序。

如果插件没有出现在列表里，先看插件面板顶部的“诊断”。损坏的 `plugin.json`、不兼容的 `api_version`、未授权权限和运行中插件的 ID 冲突都会在这里显示。

## 推荐目录结构

```text
plugins\my.serial-analyzer\
  plugin.json       插件声明文件
  main.lua          实时插件入口
  replay.lua        可选：回放解析器入口
  README.md         可选：插件说明
```

正式发布主程序时，建议不要把个人脚本、设备专用脚本或测试脚本放进默认 `plugins\` 目录。它们更适合放在独立脚本仓库，由用户按需下载。

## 两种运行角色

### 实时插件：main.lua

实时插件用于正常运行时：

- 打开或关闭串口。
- 发送串口数据。
- 监听 DataBus 事件。
- 创建动态面板。
- 创建图表或表单。
- 定时执行任务。
- 把串口 RX 解析为 `protocol.*` 事件。

典型链路：

```text
串口 RX -> main.lua 解析 -> ctx.bus.publish("protocol.xxx", {...}) -> 图表
```

### 回放解析器：replay.lua

回放解析器只用于回放阶段，负责把历史 `transport.serial.*` 事件重新解析成 `protocol.*` 事件。

它不能打开串口，不能发送数据，不能创建 UI，也不能启动真实 timer。它只做一件事：

```text
历史 raw event -> derived protocol event
```

典型链路：

```text
录制文件里的 transport.serial.default.rx
  -> replay.lua 解析
  -> ctx.replay.emit("protocol.xxx", {...})
  -> 图表
```

## Topic 命名建议

插件输出的分析结果建议放到 `protocol.<插件或协议>.*` 下，例如：

```text
protocol.pid.sample
protocol.imu.attitude
protocol.demo.sample
```

图表面板可以通过 `topic_prefix` 订阅一组相关事件：

```lua
ctx.ui.create_chart({
  id = "my-plugin.chart",
  title = "我的图表",
  topic_prefix = "protocol.my-plugin."
})
```

## 最小实时插件

```lua
ctx.log.info("hello plugin started: " .. ctx.plugin.id)

ctx.ui.create_form({
  id = "hello-form",
  title = "Hello 参数",
  auto_apply = true,
  fields = {
    { id = "message", label = "消息", kind = "text", default = "hello" }
  }
})

ctx.bus.on("ui.form.changed", function(event)
  if event.payload.panel_id ~= "hello-form" then
    return
  end

  local message = event.payload.values.message or ""
  ctx.log.info("message changed: " .. tostring(message))
end)

on_disable(function()
  ctx.ui.remove_panel("hello-form")
  ctx.log.info("hello plugin stopped")
end)
```

## 插件开发流程

1. 复制模板目录，例如 `plugins\template.hello`。
2. 修改目录名，例如 `plugins\my.first-plugin`。
3. 修改 `plugin.json`。
4. 修改 `main.lua`。
5. 在应用的插件页刷新或重启应用。
6. 启用插件。
7. 查看日志面板和动态面板是否正常。

模板里的 `plugin.json` 已经带有 `$schema`。只要 `plugins\plugin.schema.json` 和插件目录保持在同一个 `plugins\` 下，支持 JSON Schema 的编辑器会自动提示字段、权限、UI 插槽和常见枚举值。

项目根目录的 `.luarc.json` 会让 LuaLS 读取 `plugins\.lua\` 下的宿主 API stub。这样编辑 `main.lua` / `replay.lua` 时，`ctx`、`on_disable`、`on_replay_event`、`require("hw.codec")` 和 `require("hw.utils")` 不会被误报为未定义。

改完模板后，可以用仓库测试快速检查内置插件声明：

```powershell
cargo test -p tool-extension bundled_plugin_manifests_are_valid
```

`plugin.json` 里最重要的字段：

- `id` 必须唯一，发布后尽量不要修改。
- `name` 是 UI 显示名。
- `version` 用于说明插件版本。
- `api_version` 用于说明插件依赖的宿主 API 版本；当前写 `"0.1"`。
- `permissions` 只声明真正需要的权限。

## 常见设计建议

### 插件 ID 稳定

`id` 一旦发布，尽量不要修改。建议使用命名空间风格：

```text
yourname.pid-tuner
vendor.imu-viewer
project.gcode-sender
```

### 面板 ID 也要稳定

动态面板 ID 建议带上插件 ID，避免冲突：

```lua
ctx.ui.create_chart({
  id = "yourname.my-analyzer.chart",
  title = "分析图表",
  topic_prefix = "protocol.yourname."
})
```

### UI 入口优先注册命令

`contributes.ui[].command` 会触发 `ctx.commands.register` 注册的 handler。插件应该这样写：

```lua
ctx.commands.register("yourname.my-plugin.run", function(payload)
  local send = (payload.context or {}).send or {}
  ctx.log.info("run: " .. tostring(send.input))
end)
```

### 实时插件和回放解析器不要混用职责

不要在 `replay.lua` 里尝试做串口操作。回放解析器是无副作用的纯分析器。

### 退出时清理面板和定时器

```lua
on_disable(function()
  ctx.timer.cancel(timer_id)
  ctx.ui.remove_panel("my-panel")
end)
```

### 更新宿主 API 后同步 stub

如果 Rust 侧新增或改名了 `ctx.*` API，同步更新 `plugins\.lua\hardware-workbench.lua`。如果新增了可 `require` 的内置模块，同步在 `plugins\.lua\` 下补模块 stub。
